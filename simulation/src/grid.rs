use protocol::{AIR, CHUNK_EDGE, WORLD_SIZE};
use std::collections::{BTreeMap, BTreeSet};

/// Only resident sections consume memory. Missing sections are solid/unknown.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub blocks: BTreeMap<[i32; 3], Vec<u8>>,
    pub equipment: BTreeMap<u64, protocol::EquipmentSnapshot>,
}
impl Grid {
    /// A known empty legacy spawn area, also useful for isolated simulations.
    pub fn empty() -> Self {
        Self {
            blocks: Self::origins().map(|o| (o, vec![AIR; 4096])).collect(),
            equipment: BTreeMap::new(),
        }
    }
    pub fn unknown() -> Self {
        Self {
            blocks: BTreeMap::new(),
            equipment: BTreeMap::new(),
        }
    }
    /// Index into the legacy dense save layout, not the streaming world bounds.
    pub fn index(p: [i32; 3]) -> Option<usize> {
        if p.iter().zip(WORLD_SIZE).any(|(&v, n)| v < 0 || v >= n) {
            return None;
        }
        Some((p[0] + WORLD_SIZE[0] * (p[2] + WORLD_SIZE[2] * p[1])) as usize)
    }
    pub fn origin(p: [i32; 3]) -> [i32; 3] {
        p.map(|v| v.div_euclid(CHUNK_EDGE) * CHUNK_EDGE)
    }
    fn local_index(p: [i32; 3]) -> usize {
        let [x, y, z] = p.map(|v| v.rem_euclid(CHUNK_EDGE) as usize);
        x + 16 * (z + 16 * y)
    }
    pub fn get(&self, p: [i32; 3]) -> u8 {
        if !protocol::valid_voxel(p) {
            return 255;
        }
        self.blocks
            .get(&Self::origin(p))
            .map_or(255, |b| b[Self::local_index(p)])
    }
    pub fn set(&mut self, p: [i32; 3], block: u8) {
        if protocol::valid_voxel(p) {
            if let Some(b) = self.blocks.get_mut(&Self::origin(p)) {
                b[Self::local_index(p)] = block;
            }
        }
    }
    pub fn solid(&self, p: [i32; 3]) -> bool {
        !matches!(self.get(p), AIR | protocol::WATER)
    }
    pub fn chunk(&self, origin: [i32; 3]) -> Vec<u8> {
        self.blocks
            .get(&origin)
            .cloned()
            .unwrap_or_else(|| vec![255; 4096])
    }
    pub fn apply_chunk(&mut self, origin: [i32; 3], blocks: &[u8]) -> bool {
        if blocks.len() != 4096
            || blocks.iter().any(|b| !protocol::valid_block(*b))
            || !protocol::valid_voxel(origin)
            || origin.iter().any(|v| v.rem_euclid(16) != 0)
        {
            return false;
        }
        self.blocks.insert(origin, blocks.to_vec());
        true
    }
    pub fn has_column(&self, c: [i32; 2]) -> bool {
        (0..protocol::WORLD_HEIGHT)
            .step_by(16)
            .all(|y| self.blocks.contains_key(&[c[0] * 16, y, c[1] * 16]))
    }
    pub fn unload_column(&mut self, c: [i32; 2]) {
        for y in (0..protocol::WORLD_HEIGHT).step_by(16) {
            self.blocks.remove(&[c[0] * 16, y, c[1] * 16]);
        }
    }
    pub fn columns(&self) -> BTreeSet<[i32; 2]> {
        self.blocks.keys().map(|p| [p[0] / 16, p[2] / 16]).collect()
    }
    pub fn nearby_known(&self, p: [f32; 3]) -> bool {
        let c = protocol::column_at(p);
        (-1..=1).all(|z| {
            (-1..=1).all(|x| {
                let c = [c[0] + x, c[1] + z];
                !protocol::valid_column(c) || self.has_column(c)
            })
        })
    }
    pub fn legacy_blocks(&self) -> Vec<u8> {
        (0..32)
            .flat_map(|y| (0..64).flat_map(move |z| (0..64).map(move |x| self.get([x, y, z]))))
            .collect()
    }
    pub fn from_legacy(blocks: &[u8]) -> Self {
        let mut grid = Self::empty();
        for y in 0..32 {
            for z in 0..64 {
                for x in 0..64 {
                    grid.set([x, y, z], blocks[Self::index([x, y, z]).unwrap()]);
                }
            }
        }
        grid
    }
    /// Original spawn area's sections (offline demo and old save migration).
    pub fn origins() -> impl Iterator<Item = [i32; 3]> {
        (0..protocol::WORLD_HEIGHT).step_by(16).flat_map(|y| {
            (0..64)
                .step_by(16)
                .flat_map(move |z| (0..64).step_by(16).map(move |x| [x, y, z]))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_sections_unload_and_legacy_layout() {
        let mut g = Grid::unknown();
        assert!(g.solid([-1, 1, -17]));
        assert!(g.apply_chunk([-16, 0, -32], &vec![AIR; 4096]));
        g.set([-1, 1, -17], protocol::STONE);
        assert_eq!(g.get([-1, 1, -17]), protocol::STONE);
        assert_eq!(g.get([-16, 1, -32]), AIR);
        g.unload_column([-1, -2]);
        assert!(g.blocks.is_empty());
        assert!(!g.apply_chunk([i32::MAX, 0, 0], &vec![AIR; 4096]));
        let mut g = Grid::empty();
        g.set([17, 25, 49], protocol::CHEST);
        assert_eq!(Grid::from_legacy(&g.legacy_blocks()), g);
    }
}
