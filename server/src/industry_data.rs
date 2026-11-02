//! Persistent equipment facts, committed alongside terrain, players and entities.
use crate::archive_io::{self, MappedArchive};
use anyhow::{Context, Result};
use game_simulation::{equipment::occupied_cells, grid::Grid};
use protocol::{EquipmentKind, EquipmentSnapshot, MaterialAmount, AIR, MAX_EQUIPMENT};
use rkyv::{rancor::Error, Archive, Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub const DATA_VERSION: u32 = 6;
const MAGIC: &[u8; 8] = b"DNGIND06";

/// External transfers are explicit; energy includes the fluid's gravitational energy.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ExchangeLedger {
    pub injected_moles: f64,
    pub drained_moles: f64,
    pub injected_energy_j: f64,
    pub drained_energy_j: f64,
    pub pump_work_j: f64,
    pub hydro_injected_moles: [f64; 2],
    pub hydro_drained_moles: [f64; 2],
    pub hydro_injected_energy_j: f64,
    pub hydro_drained_energy_j: f64,
    pub heat_added_j: f64,
    pub heat_removed_j: f64,
}
impl ExchangeLedger {
    pub fn valid(&self) -> bool {
        [
            self.injected_moles,
            self.drained_moles,
            self.injected_energy_j,
            self.drained_energy_j,
            self.pump_work_j,
            self.heat_added_j,
            self.heat_removed_j,
        ]
        .iter()
        .chain(&self.hydro_injected_moles)
        .chain(&self.hydro_drained_moles)
        .all(|v| v.is_finite() && *v >= 0.0 && *v <= 1e24)
            && [self.hydro_injected_energy_j, self.hydro_drained_energy_j]
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1e24)
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
    pub thermal: Option<ThermalData>,
    pub pipe_ends: Option<[u8; 2]>,
    pub tank_size: Option<[u8; 2]>,
    /// Bounded MessagePack design facts; numerical results are never archived.
    pub column_design: Option<Vec<u8>>,
}
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ThermalData {
    pub mode: String,
    pub duty_fraction: f64,
    pub target_temperature_k: f64,
}
impl ThermalData {
    fn capture(t: &protocol::ThermalSettings) -> Self {
        Self {
            mode: match t.mode {
                protocol::HeatControlMode::Off => "off",
                protocol::HeatControlMode::Manual => "manual",
                protocol::HeatControlMode::OutletTemperature => "outlet_temperature",
            }
            .into(),
            duty_fraction: t.duty_fraction,
            target_temperature_k: t.target_temperature_k,
        }
    }
    fn restore(&self) -> Result<protocol::ThermalSettings> {
        Ok(protocol::ThermalSettings {
            mode: match self.mode.as_str() {
                "off" => protocol::HeatControlMode::Off,
                "manual" => protocol::HeatControlMode::Manual,
                "outlet_temperature" => protocol::HeatControlMode::OutletTemperature,
                _ => anyhow::bail!("unknown heat control mode"),
            },
            duty_fraction: self.duty_fraction,
            target_temperature_k: self.target_temperature_k,
        })
    }
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
                EquipmentKind::Vessel => "game:vessel",
                EquipmentKind::HeatExchanger => "game:heat_exchanger",
                EquipmentKind::PipeRack => "game:pipe_rack",
                EquipmentKind::Column => "game:column",
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
            thermal: value.thermal.as_ref().map(ThermalData::capture),
            pipe_ends: value.pipe_ends,
            tank_size: value.tank_size.map(|s| [s.diameter, s.height]),
            column_design: value
                .column_design
                .as_ref()
                .map(|d| rmp_serde::to_vec(d).expect("serializable column design")),
        }
    }
    fn restore(&self) -> Result<EquipmentSnapshot> {
        let kind = match self.kind.as_str() {
            "game:tank" => EquipmentKind::Tank,
            "game:pipe" => EquipmentKind::Pipe,
            "game:pump" => EquipmentKind::Pump,
            "game:valve" => EquipmentKind::Valve,
            "game:vessel" => EquipmentKind::Vessel,
            "game:heat_exchanger" => EquipmentKind::HeatExchanger,
            "game:pipe_rack" => EquipmentKind::PipeRack,
            "game:column" => EquipmentKind::Column,
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
            thermal: self
                .thermal
                .as_ref()
                .map(ThermalData::restore)
                .transpose()?,
            reading: None,
            column_design: self
                .column_design
                .as_ref()
                .map(|bytes| -> Result<protocol::ColumnDesign> {
                    anyhow::ensure!(bytes.len() <= 1024, "oversized column design");
                    Ok(rmp_serde::from_slice(bytes)?)
                })
                .transpose()?,
            pipe_ends: self.pipe_ends,
            tank_size: self
                .tank_size
                .map(|[diameter, height]| protocol::TankSize { diameter, height }),
        };
        anyhow::ensure!(value.valid(), "invalid equipment facts");
        Ok(value)
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct IndustryArchive {
    pub data_version: u32,
    /// Internal energy is meaningful only under the property model that wrote it.
    pub thermo_revision: String,
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
            thermo_revision: crate::thermo::MODEL_REVISION.into(),
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
        anyhow::ensure!(self.thermo_revision == crate::thermo::MODEL_REVISION,
            "unsupported thermodynamic model revision; stored inventories and energy were not changed");
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
        let mut cells = BTreeMap::<[i32; 3], Vec<u64>>::new();
        for saved in &self.equipment {
            let equipment = saved.restore()?;
            anyhow::ensure!(
                equipment.id < self.next_id && equipment.revision <= self.revision,
                "future equipment identity/revision"
            );
            for cell in occupied_cells(&equipment) {
                anyhow::ensure!(
                    grid.get(cell) == AIR
                        && cells
                            .get(&cell)
                            .is_none_or(|ids| ids.iter().all(|id| records.get(id).is_some_and(
                                |old| game_simulation::equipment::rack_compatible(old, &equipment)
                            ))),
                    "equipment overlaps terrain, unknown space or another device"
                );
                cells.entry(cell).or_default().push(equipment.id);
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

// Exact version-five layout, before column designs were appended.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
struct V5Equipment {
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
    pub thermal: Option<ThermalData>,
    pub pipe_ends: Option<[u8; 2]>,
    pub tank_size: Option<[u8; 2]>,
}
#[derive(Archive, Serialize, Deserialize)]
struct V5Archive {
    data_version: u32,
    thermo_revision: String,
    generation: String,
    seed: u64,
    last_update: u64,
    revision: u64,
    next_id: u64,
    equipment: Vec<V5Equipment>,
    ledger: ExchangeLedger,
}
pub fn read_v5(path: &Path) -> Result<IndustryArchive> {
    let mapped = MappedArchive::open(path, b"DNGIND05")?;
    let old = rkyv::from_bytes::<V5Archive, Error>(mapped.payload())
        .context("decode stage-four industry archive")?;
    anyhow::ensure!(
        old.data_version == 5,
        "unsupported version-five industry archive"
    );
    anyhow::ensure!(
        old.equipment.iter().all(|e| matches!(
            e.kind.as_str(),
            "game:tank"
                | "game:pipe"
                | "game:pump"
                | "game:valve"
                | "game:vessel"
                | "game:heat_exchanger"
                | "game:pipe_rack"
        )),
        "unknown version-five equipment kind"
    );
    Ok(IndustryArchive {
        data_version: DATA_VERSION,
        thermo_revision: old.thermo_revision,
        generation: old.generation,
        seed: old.seed,
        last_update: old.last_update,
        revision: old.revision,
        next_id: old.next_id,
        ledger: old.ledger,
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
                enabled: e.enabled,
                opening: e.opening,
                pump_reversed: e.pump_reversed,
                thermal: e.thermal,
                pipe_ends: e.pipe_ends,
                tank_size: e.tank_size,
                column_design: None,
            })
            .collect(),
    })
}
#[cfg(test)]
pub(crate) fn write_v5_fixture(path: &Path, a: &IndustryArchive) -> Result<()> {
    let old = V5Archive {
        data_version: 5,
        thermo_revision: a.thermo_revision.clone(),
        generation: a.generation.clone(),
        seed: a.seed,
        last_update: a.last_update,
        revision: a.revision,
        next_id: a.next_id,
        ledger: a.ledger.clone(),
        equipment: a
            .equipment
            .iter()
            .map(|e| V5Equipment {
                id: e.id,
                revision: e.revision,
                kind: e.kind.clone(),
                origin: e.origin,
                axis: e.axis,
                contents: e.contents.clone(),
                internal_energy_j: e.internal_energy_j,
                enabled: e.enabled,
                opening: e.opening,
                pump_reversed: e.pump_reversed,
                thermal: e.thermal.clone(),
                pipe_ends: e.pipe_ends,
                tank_size: e.tank_size,
            })
            .collect(),
    };
    archive_io::write_new(path, b"DNGIND05", &rkyv::to_bytes::<Error>(&old)?)
}

// Exact version-four layout, before parameterized tank sizes were appended.
#[derive(Archive, Serialize, Deserialize)]
struct V4Equipment {
    id: u64,
    revision: u64,
    kind: String,
    origin: [i32; 3],
    axis: u8,
    contents: Vec<Component>,
    internal_energy_j: f64,
    enabled: bool,
    opening: f64,
    pump_reversed: bool,
    thermal: Option<ThermalData>,
    pipe_ends: Option<[u8; 2]>,
}
#[derive(Archive, Serialize, Deserialize)]
struct V4Archive {
    data_version: u32,
    thermo_revision: String,
    generation: String,
    seed: u64,
    last_update: u64,
    revision: u64,
    next_id: u64,
    equipment: Vec<V4Equipment>,
    ledger: ExchangeLedger,
}
pub fn read_v4(path: &Path) -> Result<IndustryArchive> {
    let mapped = MappedArchive::open(path, b"DNGIND04")?;
    let old = rkyv::from_bytes::<V4Archive, Error>(mapped.payload())
        .context("decode stage-four industry archive")?;
    anyhow::ensure!(
        old.data_version == 4,
        "unsupported version-four industry archive"
    );
    anyhow::ensure!(
        old.equipment.iter().all(|e| matches!(
            e.kind.as_str(),
            "game:tank"
                | "game:pipe"
                | "game:pump"
                | "game:valve"
                | "game:vessel"
                | "game:heat_exchanger"
                | "game:pipe_rack"
        )),
        "unknown version-four equipment kind"
    );
    Ok(IndustryArchive {
        data_version: DATA_VERSION,
        thermo_revision: old.thermo_revision,
        generation: old.generation,
        seed: old.seed,
        last_update: old.last_update,
        revision: old.revision,
        next_id: old.next_id,
        ledger: old.ledger,
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
                enabled: e.enabled,
                opening: e.opening,
                pump_reversed: e.pump_reversed,
                thermal: e.thermal,
                pipe_ends: e.pipe_ends,
                tank_size: None,
                column_design: None,
            })
            .collect(),
    })
}
#[cfg(test)]
pub(crate) fn write_v4_fixture(path: &Path, a: &IndustryArchive) -> Result<()> {
    let old = V4Archive {
        data_version: 4,
        thermo_revision: a.thermo_revision.clone(),
        generation: a.generation.clone(),
        seed: a.seed,
        last_update: a.last_update,
        revision: a.revision,
        next_id: a.next_id,
        ledger: a.ledger.clone(),
        equipment: a
            .equipment
            .iter()
            .map(|e| V4Equipment {
                id: e.id,
                revision: e.revision,
                kind: e.kind.clone(),
                origin: e.origin,
                axis: e.axis,
                contents: e.contents.clone(),
                internal_energy_j: e.internal_energy_j,
                enabled: e.enabled,
                opening: e.opening,
                pump_reversed: e.pump_reversed,
                thermal: e.thermal.clone(),
                pipe_ends: e.pipe_ends,
            })
            .collect(),
    };
    archive_io::write_new(path, b"DNGIND04", &rkyv::to_bytes::<Error>(&old)?)
}

// Exact version-three layout, before explicit routed pipe ends were appended.
#[derive(Archive, Serialize, Deserialize)]
struct V3Equipment {
    id: u64,
    revision: u64,
    kind: String,
    origin: [i32; 3],
    axis: u8,
    contents: Vec<Component>,
    internal_energy_j: f64,
    enabled: bool,
    opening: f64,
    pump_reversed: bool,
    thermal: Option<ThermalData>,
}
#[derive(Archive, Serialize, Deserialize)]
struct V3Archive {
    data_version: u32,
    thermo_revision: String,
    generation: String,
    seed: u64,
    last_update: u64,
    revision: u64,
    next_id: u64,
    equipment: Vec<V3Equipment>,
    ledger: ExchangeLedger,
}
pub fn read_v3(path: &Path) -> Result<IndustryArchive> {
    let mapped = MappedArchive::open(path, b"DNGIND03")?;
    let old = rkyv::from_bytes::<V3Archive, Error>(mapped.payload())
        .context("decode stage-three industry archive")?;
    anyhow::ensure!(
        old.data_version == 3,
        "unsupported version-three industry archive"
    );
    anyhow::ensure!(
        old.equipment.iter().all(|e| matches!(
            e.kind.as_str(),
            "game:tank"
                | "game:pipe"
                | "game:pump"
                | "game:valve"
                | "game:vessel"
                | "game:heat_exchanger"
        )),
        "unknown version-three equipment kind"
    );
    Ok(IndustryArchive {
        data_version: DATA_VERSION,
        thermo_revision: old.thermo_revision,
        generation: old.generation,
        seed: old.seed,
        last_update: old.last_update,
        revision: old.revision,
        next_id: old.next_id,
        ledger: old.ledger,
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
                enabled: e.enabled,
                opening: e.opening,
                pump_reversed: e.pump_reversed,
                thermal: e.thermal,
                pipe_ends: None,
                tank_size: None,
                column_design: None,
            })
            .collect(),
    })
}
#[cfg(test)]
pub(crate) fn write_v3_fixture(path: &Path, a: &IndustryArchive) -> Result<()> {
    let old = V3Archive {
        data_version: 3,
        thermo_revision: a.thermo_revision.clone(),
        generation: a.generation.clone(),
        seed: a.seed,
        last_update: a.last_update,
        revision: a.revision,
        next_id: a.next_id,
        ledger: a.ledger.clone(),
        equipment: a
            .equipment
            .iter()
            .map(|e| V3Equipment {
                id: e.id,
                revision: e.revision,
                kind: e.kind.clone(),
                origin: e.origin,
                axis: e.axis,
                contents: e.contents.clone(),
                internal_energy_j: e.internal_energy_j,
                enabled: e.enabled,
                opening: e.opening,
                pump_reversed: e.pump_reversed,
                thermal: e.thermal.clone(),
            })
            .collect(),
    };
    archive_io::write_new(path, b"DNGIND03", &rkyv::to_bytes::<Error>(&old)?)
}

// Exact version-two layout: new rkyv fields cannot safely decode old archived bytes.
#[derive(Archive, Serialize, Deserialize)]
struct V2Ledger {
    injected_moles: f64,
    drained_moles: f64,
    injected_energy_j: f64,
    drained_energy_j: f64,
    pump_work_j: f64,
}
#[derive(Archive, Serialize, Deserialize)]
struct V2Equipment {
    id: u64,
    revision: u64,
    kind: String,
    origin: [i32; 3],
    axis: u8,
    contents: Vec<Component>,
    internal_energy_j: f64,
    enabled: bool,
    opening: f64,
    pump_reversed: bool,
}
#[derive(Archive, Serialize, Deserialize)]
struct V2Archive {
    data_version: u32,
    generation: String,
    seed: u64,
    last_update: u64,
    revision: u64,
    next_id: u64,
    equipment: Vec<V2Equipment>,
    ledger: V2Ledger,
}
pub fn read_v2(path: &Path) -> Result<IndustryArchive> {
    let mapped = MappedArchive::open(path, b"DNGIND02")?;
    let old = rkyv::from_bytes::<V2Archive, Error>(mapped.payload())
        .context("decode stage-two industry archive")?;
    anyhow::ensure!(old.data_version == 2, "unsupported legacy industry version");
    anyhow::ensure!(
        old.equipment.iter().all(|e| matches!(
            e.kind.as_str(),
            "game:tank" | "game:pipe" | "game:pump" | "game:valve"
        )),
        "unknown version-two equipment kind"
    );
    Ok(IndustryArchive {
        data_version: DATA_VERSION,
        thermo_revision: crate::thermo::MODEL_REVISION.into(),
        generation: old.generation,
        seed: old.seed,
        last_update: old.last_update,
        revision: old.revision,
        next_id: old.next_id,
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
                enabled: e.enabled,
                opening: e.opening,
                pump_reversed: e.pump_reversed,
                thermal: None,
                pipe_ends: None,
                tank_size: None,
                column_design: None,
            })
            .collect(),
        ledger: ExchangeLedger {
            injected_moles: old.ledger.injected_moles,
            drained_moles: old.ledger.drained_moles,
            injected_energy_j: old.ledger.injected_energy_j,
            drained_energy_j: old.ledger.drained_energy_j,
            pump_work_j: old.ledger.pump_work_j,
            ..Default::default()
        },
    })
}
#[cfg(test)]
pub(crate) fn write_v2_fixture(path: &Path, a: &IndustryArchive) -> Result<()> {
    let old = V2Archive {
        data_version: 2,
        generation: a.generation.clone(),
        seed: a.seed,
        last_update: a.last_update,
        revision: a.revision,
        next_id: a.next_id,
        equipment: a
            .equipment
            .iter()
            .map(|e| V2Equipment {
                id: e.id,
                revision: e.revision,
                kind: e.kind.clone(),
                origin: e.origin,
                axis: e.axis,
                contents: e.contents.clone(),
                internal_energy_j: e.internal_energy_j,
                enabled: e.enabled,
                opening: e.opening,
                pump_reversed: e.pump_reversed,
            })
            .collect(),
        ledger: V2Ledger {
            injected_moles: a.ledger.injected_moles,
            drained_moles: a.ledger.drained_moles,
            injected_energy_j: a.ledger.injected_energy_j,
            drained_energy_j: a.ledger.drained_energy_j,
            pump_work_j: a.ledger.pump_work_j,
        },
    };
    archive_io::write_new(path, b"DNGIND02", &rkyv::to_bytes::<Error>(&old)?)
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
        thermo_revision: crate::thermo::MODEL_REVISION.into(),
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
                thermal: None,
                pipe_ends: None,
                tank_size: None,
                column_design: None,
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
