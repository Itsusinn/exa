//! Persistent equipment facts, committed alongside terrain, players and entities.
use crate::archive_io::{self, MappedArchive};
use anyhow::{Context, Result};
use game_simulation::{equipment::occupied_cells, grid::Grid};
use protocol::{EquipmentKind, EquipmentSnapshot, MaterialAmount, AIR, MAX_EQUIPMENT};
use rkyv::{rancor::Error, Archive, Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const DATA_VERSION: u32 = 2;
const MAGIC: &[u8; 8] = b"DNGIND02";

/// External transfers are explicit; energy includes the fluid's gravitational energy.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ExchangeLedger {
    pub injected_moles: f64,
    pub drained_moles: f64,
    pub injected_energy_j: f64,
    pub drained_energy_j: f64,
    pub pump_work_j: f64,
}
impl ExchangeLedger {
    pub fn valid(&self) -> bool {
        [
            self.injected_moles,
            self.drained_moles,
            self.injected_energy_j,
            self.drained_energy_j,
            self.pump_work_j,
        ]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0 && *v <= 1e24)
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Component {
    pub material: String,
    pub moles: f64,
}
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EquipmentData {
    pub id: u64,
    pub revision: u64,
    /// Stable name; never the enum ordinal or a scene/ECS handle.
    pub kind: String,
    pub origin: [i32; 3],
    pub axis: u8,
    pub contents: Vec<Component>,
    pub internal_energy_j: f64,
    pub enabled: bool,
    pub opening: f64,
    pub pump_reversed: bool,
}
impl EquipmentData {
    fn capture(value: &EquipmentSnapshot) -> Self {
        Self {
            id: value.id,
            revision: value.revision,
            kind: match value.kind {
                EquipmentKind::Tank => "game:tank",
                EquipmentKind::Pipe => "game:pipe",
                EquipmentKind::Pump => "game:pump",
                EquipmentKind::Valve => "game:valve",
            }
            .into(),
            origin: value.origin,
            axis: value.axis,
            contents: value
                .contents
                .iter()
                .map(|c| Component {
                    material: c.material.clone(),
                    moles: c.moles,
                })
                .collect(),
            internal_energy_j: value.internal_energy_j,
            enabled: value.enabled,
            opening: value.opening,
            pump_reversed: value.pump_reversed,
        }
    }
    fn restore(&self) -> Result<EquipmentSnapshot> {
        let kind = match self.kind.as_str() {
            "game:tank" => EquipmentKind::Tank,
            "game:pipe" => EquipmentKind::Pipe,
            "game:pump" => EquipmentKind::Pump,
            "game:valve" => EquipmentKind::Valve,
            _ => anyhow::bail!("unknown saved equipment kind"),
        };
        let value = EquipmentSnapshot {
            id: self.id,
            revision: self.revision,
            kind,
            origin: self.origin,
            axis: self.axis,
            contents: self
                .contents
                .iter()
                .map(|c| MaterialAmount {
                    material: c.material.clone(),
                    moles: c.moles,
                })
                .collect(),
            internal_energy_j: self.internal_energy_j,
            enabled: self.enabled,
            opening: self.opening,
            pump_reversed: self.pump_reversed,
            reading: None,
        };
        anyhow::ensure!(value.valid(), "invalid equipment facts");
        Ok(value)
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct IndustryArchive {
    pub data_version: u32,
    pub generation: String,
    pub seed: u64,
    pub last_update: u64,
    pub revision: u64,
    pub next_id: u64,
    pub equipment: Vec<EquipmentData>,
    pub ledger: ExchangeLedger,
}
impl IndustryArchive {
    pub fn capture(
        grid: &Grid,
        seed: u64,
        last_update: u64,
        generation: String,
        revision: u64,
        next_id: u64,
        ledger: ExchangeLedger,
    ) -> Result<Self> {
        anyhow::ensure!(
            grid.equipment
                .iter()
                .all(|(id, equipment)| *id == equipment.id),
            "equipment key mismatch"
        );
        let archive = Self {
            data_version: DATA_VERSION,
            generation,
            seed,
            last_update,
            revision,
            next_id,
            equipment: grid
                .equipment
                .values()
                .map(EquipmentData::capture)
                .collect(),
            ledger,
        };
        archive.restore(grid)?;
        Ok(archive)
    }
    pub fn restore(&self, grid: &Grid) -> Result<BTreeMap<u64, EquipmentSnapshot>> {
        anyhow::ensure!(
            self.data_version == DATA_VERSION,
            "unsupported industry archive version"
        );
        anyhow::ensure!(self.ledger.valid(), "invalid industry exchange ledger");
        anyhow::ensure!(
            uuid::Uuid::parse_str(&self.generation)?.to_string() == self.generation,
            "invalid industry generation"
        );
        anyhow::ensure!(
            self.revision <= i64::MAX as u64
                && self.next_id > 0
                && self.next_id <= i64::MAX as u64
                && self.equipment.len() <= MAX_EQUIPMENT,
            "invalid industry counters"
        );
        let mut records = BTreeMap::new();
        let mut cells = BTreeSet::new();
        for saved in &self.equipment {
            let equipment = saved.restore()?;
            anyhow::ensure!(
                equipment.id < self.next_id && equipment.revision <= self.revision,
                "future equipment identity/revision"
            );
            for cell in occupied_cells(&equipment) {
                anyhow::ensure!(
                    grid.get(cell) == AIR && cells.insert(cell),
                    "equipment overlaps terrain, unknown space or another device"
                );
            }
            anyhow::ensure!(
                records.insert(equipment.id, equipment).is_none(),
                "duplicate equipment identity"
            );
        }
        Ok(records)
    }
}

pub fn write_new(path: &Path, archive: &IndustryArchive) -> Result<()> {
    let bytes = rkyv::to_bytes::<Error>(archive).context("encode industry archive")?;
    archive_io::write_new(path, MAGIC, &bytes)
}
pub fn read(path: &Path) -> Result<IndustryArchive> {
    let mapped = MappedArchive::open(path, MAGIC)?;
    rkyv::from_bytes::<IndustryArchive, Error>(mapped.payload()).context("decode industry archive")
}

// Keep the exact stage-one rkyv layout; changing a struct cannot decode old bytes safely.
#[derive(Archive, Serialize, Deserialize)]
struct LegacyEquipment {
    id: u64,
    revision: u64,
    kind: String,
    origin: [i32; 3],
    axis: u8,
    contents: Vec<Component>,
    internal_energy_j: f64,
}
#[derive(Archive, Serialize, Deserialize)]
struct LegacyArchive {
    data_version: u32,
    generation: String,
    seed: u64,
    last_update: u64,
    revision: u64,
    next_id: u64,
    equipment: Vec<LegacyEquipment>,
}
pub fn read_legacy(path: &Path) -> Result<IndustryArchive> {
    let mapped = MappedArchive::open(path, b"DNGIND01")?;
    let old = rkyv::from_bytes::<LegacyArchive, Error>(mapped.payload())
        .context("decode stage-one industry archive")?;
    anyhow::ensure!(old.data_version == 1, "unsupported legacy industry version");
    anyhow::ensure!(
        old.equipment
            .iter()
            .all(|e| matches!(e.kind.as_str(), "game:tank" | "game:pipe")),
        "unknown legacy equipment kind"
    );
    Ok(IndustryArchive {
        data_version: DATA_VERSION,
        generation: old.generation,
        seed: old.seed,
        last_update: old.last_update,
        revision: old.revision,
        next_id: old.next_id,
        ledger: ExchangeLedger::default(),
        equipment: old
            .equipment
            .into_iter()
            .map(|e| EquipmentData {
                id: e.id,
                revision: e.revision,
                kind: e.kind,
                origin: e.origin,
                axis: e.axis,
                contents: e.contents,
                internal_energy_j: e.internal_energy_j,
                enabled: false,
                opening: 1.0,
                pump_reversed: false,
            })
            .collect(),
    })
}

#[cfg(test)]
pub(crate) fn write_legacy_fixture(path: &Path, archive: &IndustryArchive) -> Result<()> {
    let old = LegacyArchive {
        data_version: 1,
        generation: archive.generation.clone(),
        seed: archive.seed,
        last_update: archive.last_update,
        revision: archive.revision,
        next_id: archive.next_id,
        equipment: archive
            .equipment
            .iter()
            .map(|e| LegacyEquipment {
                id: e.id,
                revision: e.revision,
                kind: e.kind.clone(),
                origin: e.origin,
                axis: e.axis,
                contents: e.contents.clone(),
                internal_energy_j: e.internal_energy_j,
            })
            .collect(),
    };
    archive_io::write_new(path, b"DNGIND01", &rkyv::to_bytes::<Error>(&old)?)
}
