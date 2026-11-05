//! Persistent living entities inspired by Minecraft Entity_format.
//! Game-native rkyv records; neither NBT nor serialized hecs handles.
use crate::archive_io::{self, MappedArchive};
use anyhow::{Context, Result};
use exa_protocol::{Actor3d, Body, CHUNK_EDGE, WORLD_SIZE};
use rkyv::{rancor::Error, Archive, Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

pub const DATA_VERSION: u32 = 2;
const MAGIC: &[u8; 8] = b"DNGENT01";

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EntityData {
    pub id: String,
    pub uuid: [u8; 16],
    /// Stable protocol ID in the NPC namespace, never a hecs entity handle.
    pub network_id: u32,
    pub pos: [f64; 3],
    /// Metres per second, matching the shared simulation.
    pub motion: [f64; 3],
    /// Yaw/pitch in radians; current zombies have no pitch.
    pub rotation: [f32; 2],
    pub on_ground: bool,
    pub health: i32,
    pub max_health: i32,
    /// Absolute simulation tick; restored together with the world's clock.
    pub attack_tick: u64,
}

impl EntityData {
    pub fn capture(uuid: [u8; 16], actor: &Actor3d, body: Body, attack_tick: u64) -> Result<Self> {
        anyhow::ensure!(actor.kind == 1, "unsupported runtime entity kind");
        Ok(Self {
            id: "game:zombie".into(),
            uuid,
            network_id: actor.id,
            pos: body.position.map(f64::from),
            motion: body.velocity.map(f64::from),
            rotation: [actor.yaw, 0.0],
            on_ground: body.grounded,
            health: actor.hp,
            max_health: actor.max_hp,
            attack_tick,
        })
    }

    fn validate(&self, tick: u64) -> Result<()> {
        anyhow::ensure!(self.id == "game:zombie", "unknown entity type");
        anyhow::ensure!(self.uuid != [0; 16], "nil entity UUID");
        anyhow::ensure!(self.network_id >= 1 << 31, "invalid NPC network ID");
        anyhow::ensure!(
            exa_protocol::valid_position(self.pos.map(|v| v as f32)),
            "invalid entity Pos"
        );
        anyhow::ensure!(
            self.motion
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 100.0),
            "invalid entity Motion"
        );
        anyhow::ensure!(
            self.rotation[0].is_finite()
                && self.rotation[0].abs() <= std::f32::consts::TAU
                && self.rotation[1] == 0.0,
            "invalid entity Rotation"
        );
        anyhow::ensure!(
            self.max_health == 30 && (1..=self.max_health).contains(&self.health),
            "invalid entity Health"
        );
        anyhow::ensure!(self.attack_tick <= tick, "future entity attack tick");
        Ok(())
    }

    pub fn restore(&self, tick: u64) -> Result<(Actor3d, Body)> {
        self.validate(tick)?;
        let body = Body {
            position: self.pos.map(|v| v as f32),
            velocity: self.motion.map(|v| v as f32),
            grounded: self.on_ground,
        };
        Ok((
            Actor3d {
                id: self.network_id,
                kind: 1,
                position: body.position,
                yaw: self.rotation[0],
                hp: self.health,
                max_hp: self.max_health,
            },
            body,
        ))
    }

    fn chunk(&self) -> [i32; 2] {
        [self.pos[0], self.pos[2]].map(|v| (v / f64::from(CHUNK_EDGE)).floor() as i32)
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EntityChunk {
    pub position: [i32; 2],
    pub entities: Vec<EntityData>,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EntityArchive {
    pub data_version: u32,
    pub dimension: String,
    pub generation: String,
    pub seed: u64,
    pub last_update: u64,
    pub chunks: Vec<EntityChunk>,
}

impl EntityArchive {
    pub fn capture(
        entities: Vec<EntityData>,
        seed: u64,
        generation: String,
        tick: u64,
    ) -> Result<Self> {
        let columns = (0..4)
            .flat_map(|z| (0..4).map(move |x| [x, z]))
            .chain(entities.iter().map(|e| e.chunk()))
            .collect();
        Self::capture_columns(entities, seed, generation, tick, columns)
    }

    pub fn capture_columns(
        mut entities: Vec<EntityData>,
        seed: u64,
        generation: String,
        tick: u64,
        columns: std::collections::BTreeSet<[i32; 2]>,
    ) -> Result<Self> {
        for entity in &entities {
            entity.validate(tick)?;
        }
        entities.sort_by_key(|e| e.network_id);
        let mut chunks = Vec::new();
        for [x, z] in columns {
            chunks.push(EntityChunk {
                position: [x, z],
                entities: entities
                    .iter()
                    .filter(|e| e.chunk() == [x, z])
                    .cloned()
                    .collect(),
            });
        }
        chunks.sort_by_key(|c| (c.position[1], c.position[0]));
        let archive = Self {
            data_version: DATA_VERSION,
            dimension: crate::player_data::DIMENSION.into(),
            generation,
            seed,
            last_update: tick,
            chunks,
        };
        anyhow::ensure!(
            archive
                .chunks
                .iter()
                .map(|c| c.entities.len())
                .sum::<usize>()
                == entities.len(),
            "entity outside generated columns"
        );
        archive.validate()?;
        Ok(archive)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            matches!(self.data_version, 1 | DATA_VERSION)
                && self.dimension == crate::player_data::DIMENSION
                && self.last_update <= i64::MAX as u64,
            "unsupported entity archive metadata"
        );
        anyhow::ensure!(
            uuid::Uuid::parse_str(&self.generation)?.to_string() == self.generation,
            "invalid entity generation"
        );
        let nx = WORLD_SIZE[0] / CHUNK_EDGE;
        let nz = WORLD_SIZE[2] / CHUNK_EDGE;
        anyhow::ensure!(
            self.data_version != 1 || self.chunks.len() == (nx * nz) as usize,
            "missing entity chunks"
        );
        anyhow::ensure!(
            self.chunks.iter().map(|c| c.entities.len()).sum::<usize>() <= 10_000,
            "too many living entities"
        );
        let mut chunks = HashSet::new();
        let mut uuids = HashSet::new();
        let mut ids = HashSet::new();
        for chunk in &self.chunks {
            let [x, z] = chunk.position;
            anyhow::ensure!(
                exa_protocol::valid_column([x, z])
                    && (self.data_version != 1 || ((0..nx).contains(&x) && (0..nz).contains(&z)))
                    && chunks.insert(chunk.position),
                "invalid/duplicate entity chunk"
            );
            for entity in &chunk.entities {
                entity.validate(self.last_update)?;
                anyhow::ensure!(entity.chunk() == chunk.position, "wrong entity chunk");
                anyhow::ensure!(
                    uuids.insert(entity.uuid) && ids.insert(entity.network_id),
                    "duplicate entity identity"
                );
            }
        }
        Ok(())
    }
}

pub fn write_new(path: &Path, archive: &EntityArchive) -> Result<()> {
    archive.validate()?;
    let bytes = rkyv::to_bytes::<Error>(archive).context("serialize entities")?;
    archive_io::write_new(path, MAGIC, &bytes)
}

pub fn read(path: &Path) -> Result<EntityArchive> {
    let map = MappedArchive::open(path, MAGIC)?;
    let archived = rkyv::access::<ArchivedEntityArchive, Error>(map.payload())
        .context("validate entity archive bytes")?;
    let value =
        rkyv::deserialize::<EntityArchive, Error>(archived).context("deserialize entities")?;
    value.validate()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EntityArchive {
        EntityArchive::capture(
            vec![EntityData {
                id: "game:zombie".into(),
                uuid: [7; 16],
                network_id: 1 << 31,
                pos: [17.25, 8.0, 49.5],
                motion: [1.0, -2.0, 0.5],
                rotation: [1.25, 0.0],
                on_ground: false,
                health: 17,
                max_health: 30,
                attack_tick: 91,
            }],
            42,
            uuid::Uuid::new_v4().to_string(),
            100,
        )
        .unwrap()
    }

    #[test]
    fn archive_round_trip_and_corrupt_bytes_are_rejected() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/entity-tests");
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let path = dir.path().join("entities.rkyv");
        let archive = sample();
        write_new(&path, &archive).unwrap();
        assert_eq!(read(&path).unwrap(), archive);
        assert!(
            write_new(&path, &archive).is_err(),
            "immutable generations cannot be overwritten"
        );
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert!(read(&path).is_err());
        // Preserve the envelope but corrupt the rkyv root and its pointers.
        let mut damaged = bytes;
        damaged[16..].fill(0xff);
        std::fs::write(&path, damaged).unwrap();
        assert!(read(&path).is_err());
    }

    #[test]
    fn validation_rejects_unknown_types_bad_fields_and_duplicate_identities() {
        let original = sample();
        // [x=1,z=3] is index 13; verify asymmetric chunk ownership.
        assert_eq!(original.chunks[13].position, [1, 3]);
        assert_eq!(original.chunks[13].entities.len(), 1);
        let mutations: &[fn(&mut EntityData)] = &[
            |e| e.id = "game:unknown".into(),
            |e| e.uuid = [0; 16],
            |e| e.network_id = 1,
            |e| e.pos[0] = f64::NAN,
            |e| e.pos[1] = -1.0,
            |e| e.pos[2] = f64::from(exa_protocol::WORLD_LIMIT),
            |e| e.motion[0] = f64::INFINITY,
            |e| e.motion[1] = 101.0,
            |e| e.rotation[0] = f32::NAN,
            |e| e.rotation[1] = 0.1,
            |e| e.health = 0,
            |e| e.health = 31,
            |e| e.max_health = 0,
            |e| e.attack_tick = 101,
        ];
        for change in mutations {
            let mut archive = original.clone();
            change(&mut archive.chunks[13].entities[0]);
            assert!(
                archive.validate().is_err(),
                "accepted {:?}",
                archive.chunks[13].entities[0]
            );
        }
        for same_uuid in [true, false] {
            let mut archive = original.clone();
            let mut duplicate = archive.chunks[13].entities[0].clone();
            if same_uuid {
                duplicate.network_id += 1;
            } else {
                duplicate.uuid = [8; 16];
            }
            archive.chunks[13].entities.push(duplicate);
            assert!(archive.validate().is_err());
        }
        let mut archive = original.clone();
        archive.data_version = 1;
        archive.chunks.pop();
        assert!(
            archive.validate().is_err(),
            "missing empty chunks are not regenerated"
        );
        let mut archive = original.clone();
        archive.chunks[0].position = [1, 0];
        assert!(archive.validate().is_err());
        let mut archive = original.clone();
        let entity = archive.chunks[13].entities.pop().unwrap();
        archive.chunks[0].entities.push(entity);
        assert!(archive.validate().is_err());
        let mut archive = original;
        archive.data_version += 1;
        assert!(archive.validate().is_err());
    }
}
