//! Chunk columns and paletted 16³ sections inspired by Minecraft Chunk_format.
//! This is a versioned rkyv format, not NBT or an Anvil region file.
use crate::{
    archive_io::{self, MappedArchive},
    player_data::ItemStack,
};
use anyhow::{Context, Result};
use exa_protocol::{
    ContainerSnapshot, GroundItem3d, InventoryEntry, CHEST, CHUNK_EDGE, CONTAINER_SLOTS,
    CONTAINER_STACK_LIMIT, WORLD_SIZE,
};
use exa_simulation::grid::Grid;
use rkyv::{rancor::Error, Archive, Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

pub const DATA_VERSION: u32 = 3;
const MAGIC: &[u8; 8] = b"DNGCHK01";
const BLOCKS: usize = 4096;
const IDS: [&str; 24] = exa_protocol::BLOCK_IDS;

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BlockStates {
    pub palette: Vec<String>,
    pub data: Vec<u64>,
}
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Section {
    pub y: i32,
    pub block_states: BlockStates,
}
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BlockEntity {
    pub id: String,
    pub position: [i32; 3],
    pub revision: u64,
    pub items: Vec<ItemStack>,
}
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ItemEntity {
    pub id: u32,
    pub position: [f32; 3],
    pub item: String,
}
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Chunk {
    pub data_version: u32,
    pub x_pos: i32,
    pub z_pos: i32,
    pub y_pos: i32,
    pub status: String,
    pub last_update: u64,
    pub sections: Vec<Section>,
    pub block_entities: Vec<BlockEntity>,
    pub entities: Vec<ItemEntity>,
}
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChunkArchive {
    pub data_version: u32,
    pub size: [i32; 3],
    pub seed: u64,
    pub generation: String,
    pub last_update: u64,
    pub chunks: Vec<Chunk>,
}

pub struct RestoredChunks {
    pub terrain: Grid,
    pub containers: Vec<ContainerSnapshot>,
    pub items: Vec<GroundItem3d>,
}

fn bits(palette_len: usize) -> usize {
    (usize::BITS - (palette_len - 1).leading_zeros()).max(4) as usize
}

fn pack(indices: &[u8], palette_len: usize) -> Vec<u64> {
    if palette_len == 1 {
        return vec![];
    }
    let bits = bits(palette_len);
    let per_word = 64 / bits;
    let mut data = vec![0; indices.len().div_ceil(per_word)];
    for (i, &index) in indices.iter().enumerate() {
        data[i / per_word] |= u64::from(index) << ((i % per_word) * bits);
    }
    data
}

fn unpack(data: &[u64], palette_len: usize) -> Result<Vec<u8>> {
    anyhow::ensure!((1..=256).contains(&palette_len), "invalid palette length");
    if palette_len == 1 {
        anyhow::ensure!(
            data.is_empty(),
            "single-state section must omit packed data"
        );
        return Ok(vec![0; BLOCKS]);
    }
    let bits = bits(palette_len);
    let per_word = 64 / bits;
    anyhow::ensure!(
        data.len() == BLOCKS.div_ceil(per_word),
        "invalid packed block data length"
    );
    let mask = (1u64 << bits) - 1;
    let mut indices = Vec::with_capacity(BLOCKS);
    for (word_index, &word) in data.iter().enumerate() {
        let used = per_word.min(BLOCKS - indices.len());
        let used_bits = used * bits;
        anyhow::ensure!(
            used_bits == 64 || word >> used_bits == 0,
            "nonzero packed padding at word {word_index}"
        );
        for slot in 0..used {
            let value = (word >> (slot * bits)) & mask;
            anyhow::ensure!(value < palette_len as u64, "palette index out of bounds");
            indices.push(value as u8);
        }
    }
    Ok(indices)
}

impl BlockStates {
    fn capture(blocks: &[u8]) -> Result<Self> {
        anyhow::ensure!(
            blocks.len() == BLOCKS && blocks.iter().all(|b| (*b as usize) < IDS.len()),
            "invalid runtime section"
        );
        let mut palette_ids = blocks.to_vec();
        palette_ids.sort_unstable();
        palette_ids.dedup();
        let indices: Vec<_> = blocks
            .iter()
            .map(|b| palette_ids.binary_search(b).unwrap() as u8)
            .collect();
        Ok(Self {
            palette: palette_ids
                .iter()
                .map(|&b| IDS[b as usize].into())
                .collect(),
            data: pack(&indices, palette_ids.len()),
        })
    }
    fn restore(&self) -> Result<Vec<u8>> {
        anyhow::ensure!(
            !self.palette.is_empty() && self.palette.len() <= IDS.len(),
            "invalid block palette"
        );
        let mut used = HashSet::new();
        let palette: Vec<_> = self
            .palette
            .iter()
            .map(|name| {
                let id = IDS
                    .iter()
                    .position(|known| *known == name)
                    .context("unknown block state")? as u8;
                anyhow::ensure!(used.insert(id), "duplicate block state");
                Ok(id)
            })
            .collect::<Result<_>>()?;
        Ok(unpack(&self.data, palette.len())?
            .iter()
            .map(|&i| palette[i as usize])
            .collect())
    }
}

impl ChunkArchive {
    pub fn capture(
        terrain: &Grid,
        containers: &BTreeMap<[i32; 3], ContainerSnapshot>,
        items: &[GroundItem3d],
        seed: u64,
        generation: String,
        tick: u64,
    ) -> Result<Self> {
        anyhow::ensure!(
            terrain.columns().iter().all(|c| terrain.has_column(*c)),
            "invalid world grid length"
        );
        let mut chunks = Vec::new();
        for [x, z] in terrain.columns() {
            let mut sections = Vec::new();
            for y in 0..exa_protocol::WORLD_HEIGHT / CHUNK_EDGE {
                sections.push(Section {
                    y,
                    block_states: BlockStates::capture(&terrain.chunk([x * 16, y * 16, z * 16]))?,
                });
            }
            chunks.push(Chunk {
                data_version: DATA_VERSION,
                x_pos: x,
                z_pos: z,
                y_pos: 0,
                status: "game:full".into(),
                last_update: tick,
                sections,
                block_entities: containers
                    .values()
                    .filter(|c| {
                        c.position[0].div_euclid(16) == x && c.position[2].div_euclid(16) == z
                    })
                    .map(|c| BlockEntity {
                        id: "game:chest".into(),
                        position: c.position,
                        revision: c.revision,
                        items: c
                            .items
                            .iter()
                            .enumerate()
                            .map(|(slot, item)| ItemStack {
                                slot: slot as u8,
                                id: item.name.clone(),
                                count: item.quantity,
                            })
                            .collect(),
                    })
                    .collect(),
                entities: items
                    .iter()
                    .filter(|i| {
                        (i.position[0] / 16.0).floor() as i32 == x
                            && (i.position[2] / 16.0).floor() as i32 == z
                    })
                    .map(|i| ItemEntity {
                        id: i.id,
                        position: i.position,
                        item: i.name.clone(),
                    })
                    .collect(),
            });
        }
        anyhow::ensure!(
            chunks.iter().map(|c| c.block_entities.len()).sum::<usize>() == containers.len()
                && chunks.iter().map(|c| c.entities.len()).sum::<usize>() == items.len(),
            "entities outside saved world"
        );
        chunks.sort_by_key(|c| (c.z_pos, c.x_pos));
        let value = Self {
            data_version: DATA_VERSION,
            size: [0, exa_protocol::WORLD_HEIGHT, 0],
            seed,
            generation,
            last_update: tick,
            chunks,
        };
        value.restore()?;
        Ok(value)
    }

    pub fn restore(&self) -> Result<RestoredChunks> {
        anyhow::ensure!(
            matches!(self.data_version, 1..=DATA_VERSION)
                && self.size
                    == if self.data_version == 1 {
                        WORLD_SIZE
                    } else if self.data_version == 2 {
                        [0, 32, 0]
                    } else {
                        [0, exa_protocol::WORLD_HEIGHT, 0]
                    }
                && self.last_update <= i64::MAX as u64,
            "unsupported chunk archive metadata"
        );
        anyhow::ensure!(
            uuid::Uuid::parse_str(&self.generation)?.to_string() == self.generation,
            "invalid chunk generation"
        );
        let nx = WORLD_SIZE[0] / 16;
        let nz = WORLD_SIZE[2] / 16;
        anyhow::ensure!(
            self.data_version != 1 || self.chunks.len() == (nx * nz) as usize,
            "missing or excess chunk columns"
        );
        let mut terrain = Grid::unknown();
        let mut columns = HashSet::new();
        for chunk in &self.chunks {
            anyhow::ensure!(
                chunk.data_version == self.data_version
                    && chunk.status == "game:full"
                    && chunk.y_pos == 0
                    && chunk.last_update <= self.last_update,
                "invalid chunk metadata"
            );
            anyhow::ensure!(
                exa_protocol::valid_column([chunk.x_pos, chunk.z_pos])
                    && (self.data_version != 1
                        || ((0..nx).contains(&chunk.x_pos) && (0..nz).contains(&chunk.z_pos)))
                    && columns.insert((chunk.x_pos, chunk.z_pos)),
                "duplicate or invalid chunk position"
            );
            anyhow::ensure!(
                chunk.sections.len() == (self.size[1] / 16) as usize,
                "missing section"
            );
            let mut sections = HashSet::new();
            for section in &chunk.sections {
                anyhow::ensure!(
                    self.data_version == DATA_VERSION
                        || section
                            .block_states
                            .palette
                            .iter()
                            .all(|id| IDS[..=CHEST as usize].contains(&id.as_str())),
                    "new block state in legacy archive"
                );
                anyhow::ensure!(
                    (0..self.size[1] / 16).contains(&section.y) && sections.insert(section.y),
                    "duplicate or invalid section Y"
                );
                anyhow::ensure!(
                    terrain.apply_chunk(
                        [chunk.x_pos * 16, section.y * 16, chunk.z_pos * 16],
                        &section.block_states.restore()?
                    ),
                    "invalid decoded section"
                );
            }
            // Only validated legacy archives gain air above their original ceiling.
            // Missing sections in a current archive must still fail validation.
            for y in (self.size[1]..exa_protocol::WORLD_HEIGHT).step_by(16) {
                terrain.apply_chunk(
                    [chunk.x_pos * 16, y, chunk.z_pos * 16],
                    &vec![exa_protocol::AIR; BLOCKS],
                );
            }
        }
        let mut containers = Vec::new();
        let mut positions = HashSet::new();
        let mut items = Vec::new();
        let mut ids = HashSet::new();
        for chunk in &self.chunks {
            anyhow::ensure!(
                chunk.block_entities.len() <= (16 * 16 * self.size[1]) as usize
                    && chunk.entities.len() <= 100_000,
                "too many chunk entities"
            );
            for chest in &chunk.block_entities {
                anyhow::ensure!(
                    chest.id == "game:chest"
                        && chest.revision <= i64::MAX as u64
                        && chest.position[0].div_euclid(16) == chunk.x_pos
                        && chest.position[2].div_euclid(16) == chunk.z_pos
                        && positions.insert(chest.position)
                        && terrain.get(chest.position) == CHEST,
                    "invalid chest block entity"
                );
                anyhow::ensure!(
                    chest.items.len() <= CONTAINER_SLOTS,
                    "chest is over capacity"
                );
                let mut names = HashSet::new();
                let mut slots = HashSet::new();
                for item in &chest.items {
                    anyhow::ensure!(
                        (item.slot as usize) < chest.items.len()
                            && slots.insert(item.slot)
                            && names.insert(&item.id)
                            && !item.id.is_empty()
                            && item.id.len() <= 128
                            && (1..=CONTAINER_STACK_LIMIT).contains(&item.count),
                        "invalid chest item"
                    );
                }
                let mut stacks = chest.items.clone();
                stacks.sort_by_key(|i| i.slot);
                containers.push(ContainerSnapshot {
                    position: chest.position,
                    revision: chest.revision,
                    items: stacks
                        .into_iter()
                        .map(|i| InventoryEntry {
                            name: i.id,
                            quantity: i.count,
                        })
                        .collect(),
                });
            }
            for item in &chunk.entities {
                anyhow::ensure!(
                    ids.insert(item.id)
                        && !item.item.is_empty()
                        && item.item.len() <= 128
                        && exa_protocol::valid_position(item.position)
                        && (item.position[0] / 16.0).floor() as i32 == chunk.x_pos
                        && (item.position[2] / 16.0).floor() as i32 == chunk.z_pos,
                    "invalid ground item entity"
                );
                items.push(GroundItem3d {
                    id: item.id,
                    position: item.position,
                    name: item.item.clone(),
                });
            }
        }
        anyhow::ensure!(
            terrain
                .blocks
                .values()
                .flatten()
                .filter(|b| **b == CHEST)
                .count()
                == containers.len(),
            "chest voxel/entity count mismatch"
        );
        anyhow::ensure!(items.len() <= 100_000, "too many world items");
        containers.sort_by_key(|c| c.position);
        items.sort_by_key(|i| i.id);
        Ok(RestoredChunks {
            terrain,
            containers,
            items,
        })
    }
}

pub fn write_new(path: &Path, archive: &ChunkArchive) -> Result<()> {
    archive.restore()?;
    let bytes = rkyv::to_bytes::<Error>(archive).context("serialize chunks")?;
    archive_io::write_new(path, MAGIC, &bytes)
}

pub fn read(path: &Path) -> Result<ChunkArchive> {
    let map = MappedArchive::open(path, MAGIC)?;
    let archived = rkyv::access::<ArchivedChunkArchive, Error>(map.payload())
        .context("validate chunk archive bytes")?;
    let value = rkyv::deserialize::<ChunkArchive, Error>(archived).context("deserialize chunks")?;
    value.restore()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChunkArchive {
        let mut grid = Grid::empty();
        // Asymmetric coordinates catch X/Z swaps and Y-major indexing errors.
        for (p, block) in [
            ([1, 2, 3], 1),
            ([15, 15, 15], 3),
            ([16, 16, 17], 4),
            ([63, 31, 62], 7),
            ([32, 17, 48], CHEST),
        ] {
            grid.set(p, block);
        }
        let chest = ContainerSnapshot {
            position: [32, 17, 48],
            revision: 7,
            items: vec![InventoryEntry {
                name: "Supplies".into(),
                quantity: 123,
            }],
        };
        ChunkArchive::capture(
            &grid,
            &BTreeMap::from([(chest.position, chest)]),
            &[GroundItem3d {
                id: 17,
                position: [16.5, 18.0, 47.5],
                name: "Tool".into(),
            }],
            91,
            uuid::Uuid::new_v4().to_string(),
            123,
        )
        .unwrap()
    }

    #[test]
    fn palette_roundtrip_and_non_crossing_words() {
        let single = BlockStates::capture(&vec![3; BLOCKS]).unwrap();
        assert!(single.data.is_empty());
        assert_eq!(single.restore().unwrap(), vec![3; BLOCKS]);
        let blocks: Vec<_> = (0..BLOCKS).map(|i| (i % IDS.len()) as u8).collect();
        assert_eq!(
            BlockStates::capture(&blocks).unwrap().restore().unwrap(),
            blocks
        );
        // Five-bit indices leave four unused high bits per word.
        let indices: Vec<_> = (0..BLOCKS).map(|i| (i % 17) as u8).collect();
        let data = pack(&indices, 17);
        assert_eq!(data.len(), BLOCKS.div_ceil(12));
        assert!(data.iter().all(|v| v >> 60 == 0));
        assert_eq!(unpack(&data, 17).unwrap(), indices);
        assert!(unpack(&data[..data.len() - 1], 17).is_err());
        let mut bad = data.clone();
        bad[0] |= 1 << 63;
        assert!(unpack(&bad, 17).is_err());
        bad = data;
        bad[0] |= 31;
        assert!(unpack(&bad, 17).is_err());
        assert!(unpack(&[0], 1).is_err());
        assert!(unpack(&[], 0).is_err());
    }

    #[test]
    fn chunk_mmap_roundtrip_preserves_coordinates_and_entities() {
        let archive = sample();
        let restored = archive.restore().unwrap();
        for (p, block) in [
            ([1, 2, 3], 1),
            ([15, 15, 15], 3),
            ([16, 16, 17], 4),
            ([63, 31, 62], 7),
            ([32, 17, 48], CHEST),
        ] {
            assert_eq!(restored.terrain.get(p), block);
        }
        assert_eq!(restored.terrain.get([3, 2, 1]), 0);
        assert_eq!(restored.containers[0].items[0].quantity, 123);
        assert_eq!(restored.items[0].position, [16.5, 18.0, 47.5]);
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/chunk-tests");
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let path = dir.path().join("chunks.rkyv");
        write_new(&path, &archive).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() < restored.terrain.blocks.len() * 4096);
        assert_eq!(read(&path).unwrap(), archive);
        assert!(write_new(&path, &archive).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        for bad in [&bytes[..15], &bytes[..bytes.len() - 1]] {
            std::fs::write(&path, bad).unwrap();
            assert!(read(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bad);
        }
        let mut bad = bytes.clone();
        bad[16..].fill(255);
        std::fs::write(&path, &bad).unwrap();
        assert!(read(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bad);
    }

    #[test]
    fn invalid_or_missing_chunk_data_is_never_filled_with_air() {
        let original = sample();
        let mut bad = original.clone();
        bad.data_version = 1;
        bad.size = WORLD_SIZE;
        for c in &mut bad.chunks {
            c.data_version = 1;
        }
        bad.chunks.pop();
        assert!(bad.restore().is_err());
        bad = original.clone();
        bad.chunks[1] = bad.chunks[0].clone();
        assert!(bad.restore().is_err());
        bad = original.clone();
        bad.chunks[0].sections.pop();
        assert!(bad.restore().is_err());
        bad = original.clone();
        bad.chunks[0].sections[1].y = 0;
        assert!(bad.restore().is_err());
        bad = original.clone();
        bad.chunks[0].sections[0].block_states.palette[0] = "unknown:block".into();
        assert!(bad.restore().is_err());
        bad = original.clone();
        bad.chunks[0].sections[0]
            .block_states
            .palette
            .push("game:air".into());
        assert!(bad.restore().is_err());
        bad = original.clone();
        bad.data_version += 1;
        assert!(bad.restore().is_err());
        bad = original.clone();
        bad.chunks[14].block_entities.clear();
        assert!(bad.restore().is_err());
        bad = original.clone();
        bad.chunks[14].block_entities[0].items[0].slot = 1;
        assert!(bad.restore().is_err());
        bad = original;
        bad.chunks[9].entities[0].position[0] = 32.0;
        assert!(bad.restore().is_err());
    }
}
