use super::source::{reduce_layer, Summary};
use exa_protocol::DistantTileKey;

pub(super) struct Assembly {
    key: DistantTileKey,
    coverage: Vec<bool>,
    covered: usize,
    /// At stride32 retain the stride16 contributions until all 1024 columns
    /// are present; a parent is never synthesized from missing/AIR children.
    data: Vec<u8>,
    coarse: Option<Vec<u8>>,
}

impl Assembly {
    pub(super) fn new(key: DistantTileKey) -> Self {
        let edge = if key.stride == 32 { 32 } else { 16 };
        Self {
            key,
            coverage: vec![false; usize::from(key.stride).pow(2)],
            covered: 0,
            data: vec![0; edge * edge * 128 / usize::from(key.y_stride())],
            coarse: None,
        }
    }

    fn index(&self, column: [i32; 2]) -> usize {
        let [x, z] =
            std::array::from_fn(|i| (column[i] - self.key.origin[i].div_euclid(16)) as usize);
        x + usize::from(self.key.stride) * z
    }

    pub(super) fn has(&self, column: [i32; 2]) -> bool {
        self.coverage[self.index(column)]
    }
    pub(super) fn clear(&mut self, column: [i32; 2]) {
        let index = self.index(column);
        if self.coverage[index] {
            self.coverage[index] = false;
            self.covered -= 1;
            self.coarse = None;
        }
    }

    pub(super) fn add(&mut self, column: [i32; 2], summary: &Summary) -> bool {
        let stride = self.key.stride.min(16);
        let Some(source) = summary.get(stride) else {
            return false;
        };
        let index = self.index(column);
        if self.coverage[index] {
            return false;
        }
        let src_edge = 16 / usize::from(stride);
        let dst_edge = if self.key.stride == 32 { 32 } else { 16 };
        let ox = (column[0] - self.key.origin[0].div_euclid(16)) as usize * src_edge;
        let oz = (column[1] - self.key.origin[1].div_euclid(16)) as usize * src_edge;
        for y in 0..128 / usize::from(self.key.y_stride()) {
            for z in 0..src_edge {
                let src = src_edge * (z + src_edge * y);
                let dst = ox + dst_edge * (oz + z + dst_edge * y);
                self.data[dst..dst + src_edge].copy_from_slice(&source[src..src + src_edge]);
            }
        }
        self.coverage[index] = true;
        self.covered += 1;
        if self.covered == self.coverage.len() && self.key.stride == 32 {
            self.coarse = Some(reduce_layer(&self.data, 32, 32, false));
        }
        true
    }

    pub(super) fn blocks(&self) -> Option<&[u8]> {
        if self.covered != self.coverage.len() {
            return None;
        }
        if self.key.stride == 32 {
            self.coarse.as_deref()
        } else {
            Some(&self.data)
        }
    }
    pub(super) fn bytes(&self) -> usize {
        self.data.len() + self.coverage.len() + self.coarse.as_ref().map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exa_protocol::{AIR, PLANK};

    #[test]
    fn root_waits_for_all_1024_sources_and_replaces_air_edits() {
        let key = DistantTileKey {
            origin: [-512, -512],
            stride: 32,
        };
        let mut tile = Assembly::new(key);
        let empty = Summary::from_column(vec![AIR; 32768], 16);
        for z in -32..0 {
            for x in -32..0 {
                if [x, z] == [-1, -1] {
                    continue;
                }
                assert!(tile.add([x, z], &empty));
            }
        }
        assert!(tile.blocks().is_none());
        let mut blocks = vec![AIR; 32768];
        blocks[32767] = PLANK;
        tile.add([-1, -1], &Summary::from_column(blocks, 16));
        assert_eq!(tile.blocks().unwrap().len(), 8192);
        assert_eq!(tile.blocks().unwrap()[8191], PLANK);
        tile.clear([-1, -1]);
        assert!(tile.blocks().is_none());
        tile.add([-1, -1], &empty);
        assert!(tile.blocks().unwrap().iter().all(|&v| v == AIR));
    }
}
