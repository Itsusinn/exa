//! Exact hierarchical reduction of one fully generated/read 16x128x16 column.
//! Every source voxel participates; no ray, point or height-only approximation.
use exa_protocol::{AIR, BLOCK_IDS};

#[derive(Clone)]
pub(super) struct Summary {
    /// Ascending strides, ending at 16. Fine data is retained only when needed.
    levels: Vec<(u8, Vec<u8>)>,
}

impl Summary {
    pub(super) fn from_column(blocks: Vec<u8>, minimum: u8) -> Self {
        debug_assert_eq!(blocks.len(), 32768);
        let mut levels = Vec::new();
        let mut current = blocks;
        let mut stride = 1;
        loop {
            let next = (stride < 16).then(|| {
                reduce_layer(
                    &current,
                    16 / usize::from(stride),
                    128 / usize::from(stride.min(4)),
                    stride < 4,
                )
            });
            if stride >= minimum.min(16) {
                levels.push((stride, current));
            }
            if let Some(next) = next {
                current = next;
                stride *= 2;
            } else {
                break;
            }
        }
        Self { levels }
    }

    pub(super) fn get(&self, stride: u8) -> Option<&[u8]> {
        self.levels
            .iter()
            .find(|(s, _)| *s == stride)
            .map(|(_, blocks)| blocks.as_slice())
    }

    pub(super) fn bytes(&self) -> usize {
        self.levels.iter().map(|(_, blocks)| blocks.len()).sum()
    }
}

/// Reduce 2x2 in XZ and optionally 2 in Y. Only an entirely empty cell becomes
/// empty, preserving thin occupied structures. Stable ID order breaks ties.
pub(super) fn reduce_layer(blocks: &[u8], edge: usize, height: usize, halve_y: bool) -> Vec<u8> {
    debug_assert_eq!(blocks.len(), edge * edge * height);
    let y_scale = if halve_y { 2 } else { 1 };
    let out_edge = edge / 2;
    let mut reduced = Vec::with_capacity(out_edge * out_edge * height / y_scale);
    for y in 0..height / y_scale {
        for z in 0..out_edge {
            for x in 0..out_edge {
                let mut counts = [0_u8; BLOCK_IDS.len()];
                for dy in 0..y_scale {
                    for dz in 0..2 {
                        for dx in 0..2 {
                            let index =
                                x * 2 + dx + edge * (z * 2 + dz + edge * (y * y_scale + dy));
                            counts[blocks[index] as usize] += 1;
                        }
                    }
                }
                let (mut selected, mut most) = (AIR, 0);
                for (id, &count) in counts.iter().enumerate().skip(1) {
                    if count > most {
                        selected = id as u8;
                        most = count;
                    }
                }
                reduced.push(selected);
            }
        }
    }
    reduced
}

#[cfg(test)]
mod tests {
    use super::*;
    use exa_protocol::{PLANK, STONE};

    #[test]
    fn every_source_voxel_contributes_and_vertical_resolution_stops_at_four() {
        let mut blocks = vec![AIR; 32768];
        blocks[15 + 16 * (15 + 16 * 127)] = PLANK;
        blocks[0] = STONE;
        let summary = Summary::from_column(blocks.clone(), 1);
        assert_eq!(summary.get(1).unwrap(), blocks);
        for (stride, len) in [(2, 4096), (4, 512), (8, 128), (16, 32)] {
            let layer = summary.get(stride).unwrap();
            assert_eq!(layer.len(), len);
            assert_eq!(layer[0], STONE);
            assert_eq!(*layer.last().unwrap(), PLANK);
            assert_eq!(layer[layer.len() / 2], AIR);
        }
        let coarse = Summary::from_column(blocks, 32);
        assert_eq!(coarse.bytes(), 32);
        assert_eq!(coarse.get(16), summary.get(16));
        assert!(coarse.get(8).is_none());
    }
}
