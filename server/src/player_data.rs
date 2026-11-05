//! Versioned player records inspired by Minecraft's player.dat field structure.
//! These are rkyv archives, not NBT files or Minecraft save imports.
use anyhow::{Context, Result};
use rkyv::{rancor::Error, Archive, Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

pub const DATA_VERSION: u32 = 1;
pub const DIMENSION: &str = "game:overworld";
const MAGIC: &[u8; 8] = b"DNGPLY01";
#[cfg(test)]
const HEADER: usize = 16;

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ItemStack {
    pub slot: u8,
    /// The current game's item name is its identity; no Minecraft item registry.
    pub id: String,
    pub count: u32,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlayerData {
    pub data_version: u32,
    pub uuid: [u8; 16],
    pub name: String,
    pub dimension: String,
    pub pos: [f64; 3],
    /// Game-native metres per second (Minecraft uses metres per tick).
    pub motion: [f64; 3],
    /// Game-native yaw/pitch in radians.
    pub rotation: [f32; 2],
    pub on_ground: bool,
    pub health: f32,
    pub inventory: Vec<ItemStack>,
    pub selected_item_slot: u8,
}

impl PlayerData {
    pub fn capture(
        uuid: [u8; 16],
        name: String,
        body: exa_protocol::Body,
        health: i32,
        rotation: [f32; 2],
        inventory: &exa_protocol::PlayerInventory,
    ) -> Result<Self> {
        anyhow::ensure!(
            inventory.slots.len() == exa_protocol::INVENTORY_SLOTS,
            "invalid runtime slot count"
        );
        let value = Self {
            data_version: DATA_VERSION,
            uuid,
            name,
            dimension: DIMENSION.into(),
            pos: body.position.map(f64::from),
            motion: body.velocity.map(f64::from),
            rotation,
            on_ground: body.grounded,
            health: health as f32,
            inventory: inventory
                .slots
                .iter()
                .enumerate()
                .filter_map(|(slot, item)| {
                    item.as_ref().map(|item| ItemStack {
                        slot: slot as u8,
                        id: item.name.clone(),
                        count: item.quantity,
                    })
                })
                .collect(),
            selected_item_slot: inventory.selected,
        };
        value.validate()?;
        Ok(value)
    }

    /// Network revisions belong to a connection and restart at zero on restore.
    /// The world must still check the restored body against terrain and actors.
    pub fn restore(&self) -> Result<(exa_protocol::Body, exa_protocol::PlayerInventory)> {
        self.validate()?;
        let body = exa_protocol::Body {
            position: self.pos.map(|v| v as f32),
            velocity: self.motion.map(|v| v as f32),
            grounded: self.on_ground,
        };
        let mut inventory = exa_protocol::PlayerInventory {
            selected: self.selected_item_slot,
            ..Default::default()
        };
        for item in &self.inventory {
            inventory.slots[item.slot as usize] = Some(exa_protocol::InventoryEntry {
                name: item.id.clone(),
                quantity: item.count,
            });
        }
        Ok((body, inventory))
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.data_version == DATA_VERSION,
            "unsupported player DataVersion"
        );
        anyhow::ensure!(self.uuid != [0; 16], "nil player UUID");
        anyhow::ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 128,
            "invalid player name"
        );
        anyhow::ensure!(self.dimension == DIMENSION, "unsupported player Dimension");
        anyhow::ensure!(
            self.pos
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1_000_000.0),
            "invalid player Pos"
        );
        anyhow::ensure!(
            self.motion
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 100.0),
            "invalid player Motion"
        );
        anyhow::ensure!(
            self.rotation[0].is_finite()
                && self.rotation[0].abs() <= std::f32::consts::TAU
                && self.rotation[1].is_finite()
                && self.rotation[1].abs() <= 1.5,
            "invalid player Rotation"
        );
        anyhow::ensure!(
            self.health.is_finite() && (0.0..=100.0).contains(&self.health),
            "invalid player Health"
        );
        anyhow::ensure!(
            self.selected_item_slot < exa_protocol::HOTBAR_SLOTS as u8,
            "invalid SelectedItemSlot"
        );
        anyhow::ensure!(
            self.inventory.len() <= exa_protocol::INVENTORY_SLOTS,
            "too many inventory entries"
        );
        let mut used = HashSet::new();
        for item in &self.inventory {
            anyhow::ensure!(
                item.slot < exa_protocol::INVENTORY_SLOTS as u8 && used.insert(item.slot),
                "invalid or duplicate inventory Slot"
            );
            anyhow::ensure!(
                !item.id.is_empty()
                    && item.id.len() <= 128
                    && (1..=exa_protocol::ITEM_STACK_LIMIT).contains(&item.count),
                "invalid inventory item"
            );
        }
        Ok(())
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlayerArchive {
    pub data_version: u32,
    pub players: Vec<PlayerData>,
}

impl PlayerArchive {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.data_version == DATA_VERSION,
            "unsupported player archive version"
        );
        anyhow::ensure!(self.players.len() <= 10_000, "too many saved players");
        let mut ids = HashSet::new();
        for player in &self.players {
            player.validate()?;
            anyhow::ensure!(ids.insert(player.uuid), "duplicate player UUID");
        }
        Ok(())
    }
}

/// Create an immutable checkpoint. Existing files are never truncated.
pub fn write_new(path: &Path, value: &PlayerArchive) -> Result<()> {
    value.validate()?;
    let bytes = rkyv::to_bytes::<Error>(value).context("serialize player archive")?;
    crate::archive_io::write_new(path, MAGIC, &bytes)
}

/// Validate mapped bytes before restoring an owned, mutable runtime record.
pub fn read(path: &Path) -> Result<PlayerArchive> {
    let map = crate::archive_io::MappedArchive::open(path, MAGIC)?;
    let archived = rkyv::access::<ArchivedPlayerArchive, Error>(map.payload())
        .context("validate player archive bytes")?;
    let value = rkyv::deserialize::<PlayerArchive, Error>(archived)
        .context("deserialize player archive")?;
    value.validate()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> PlayerArchive {
        PlayerArchive {
            data_version: DATA_VERSION,
            players: vec![PlayerData {
                data_version: DATA_VERSION,
                uuid: [7; 16],
                name: "测试玩家".into(),
                dimension: DIMENSION.into(),
                pos: [28.5, 7.0, 32.5],
                motion: [0.0, -2.0, 0.0],
                rotation: [1.2, 0.4],
                on_ground: false,
                health: 72.0,
                inventory: vec![
                    ItemStack {
                        slot: 8,
                        id: "Field supplies".into(),
                        count: 64,
                    },
                    ItemStack {
                        slot: 35,
                        id: "Field supplies".into(),
                        count: 3,
                    },
                ],
                selected_item_slot: 8,
            }],
        }
    }

    fn directory() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/player-data-tests");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }

    #[test]
    fn mmap_roundtrip_preserves_sparse_slots_and_existing_files() {
        let dir = directory();
        let path = dir.path().join("players.rkyv");
        let data = example();
        write_new(&path, &data).unwrap();
        assert_eq!(read(&path).unwrap(), data);
        let player = &data.players[0];
        let (body, inventory) = player.restore().unwrap();
        assert_eq!(inventory.slots.len(), 36);
        assert!(inventory.slots[0].is_none());
        assert_eq!(inventory.slots[8].as_ref().unwrap().quantity, 64);
        assert_eq!(inventory.slots[35].as_ref().unwrap().quantity, 3);
        assert_eq!(inventory.revision, 0);
        assert_eq!(inventory.selected, 8);
        assert_eq!(
            PlayerData::capture(
                player.uuid,
                player.name.clone(),
                body,
                player.health as i32,
                player.rotation,
                &inventory
            )
            .unwrap(),
            *player
        );
        let original = std::fs::read(&path).unwrap();
        assert!(write_new(&path, &data).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let mut empty = data;
        empty.players.clear();
        let path = dir.path().join("empty.rkyv");
        write_new(&path, &empty).unwrap();
        assert_eq!(read(&path).unwrap(), empty);
    }

    #[test]
    fn invalid_records_and_corrupt_archives_are_rejected_without_rewriting() {
        let dir = directory();
        let path = dir.path().join("players.rkyv");
        let mut invalid = example();
        invalid.players[0].inventory[1].slot = 8;
        assert!(write_new(&path, &invalid).is_err());
        assert!(!path.exists());
        invalid = example();
        invalid.players[0].inventory[0].count = 65;
        assert!(invalid.validate().is_err());
        invalid = example();
        invalid.players[0].pos[0] = f64::NAN;
        assert!(invalid.validate().is_err());
        invalid = example();
        invalid.players[0].selected_item_slot = 9;
        assert!(invalid.validate().is_err());
        invalid = example();
        invalid.data_version += 1;
        assert!(invalid.validate().is_err());
        write_new(&path, &example()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        for len in [0, 7, HEADER, bytes.len() - 1] {
            std::fs::write(&path, &bytes[..len]).unwrap();
            assert!(read(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), &bytes[..len]);
        }
        let mut corrupt = bytes;
        corrupt[HEADER..].fill(0xff);
        std::fs::write(&path, &corrupt).unwrap();
        assert!(read(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    }
}
