use crate::{valid_block, valid_column, WORLD_HEIGHT, WORLD_LIMIT};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const FAR_DISTANCE: i32 = 2048;
pub const FAR_FINE_RADIUS: i32 = 4;
pub const MAX_WANTED_TILES: usize = 1024;
pub const MAX_CACHED_TILES: usize = 1536;
pub const MAX_DISTANT_CACHE_BYTES: usize = 48 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DistantTileKey {
    /// Minimum X/Z world coordinates in metres, aligned to span().
    pub origin: [i32; 2],
    pub stride: u8,
}

impl DistantTileKey {
    pub fn span(self) -> i32 {
        16 * i32::from(self.stride)
    }
    pub fn y_stride(self) -> u8 {
        self.stride.min(4)
    }
    pub fn children(self) -> Option<[Self; 4]> {
        if !matches!(self.stride, 2 | 4 | 8 | 16 | 32) || distant_tile_block_count(self).is_none() {
            return None;
        }
        let stride = self.stride / 2;
        let half = 16 * i32::from(stride);
        Some(std::array::from_fn(|i| Self {
            origin: [
                self.origin[0] + (i as i32 % 2) * half,
                self.origin[1] + (i as i32 / 2) * half,
            ],
            stride,
        }))
    }
    pub fn contains_column(self, column: [i32; 2]) -> bool {
        valid_column(column)
            && self.origin.iter().zip(column).all(|(&lo, c)| {
                let pos = i64::from(c) * 16;
                i64::from(lo) <= pos && pos < i64::from(lo) + i64::from(self.span())
            })
    }
}

#[derive(Clone, Debug, Default)]
pub struct DistantPlan {
    pub roots: Vec<DistantTileKey>,
    /// Includes ancestors as fallback coverage, not only the finest leaves.
    pub nodes: Vec<DistantTileKey>,
    pub leaves: BTreeSet<DistantTileKey>,
}

pub fn distant_tile_block_count(key: DistantTileKey) -> Option<usize> {
    if !matches!(key.stride, 1 | 2 | 4 | 8 | 16 | 32) {
        return None;
    }
    let span = key.span();
    if key
        .origin
        .iter()
        .any(|&v| v.rem_euclid(span) != 0 || v < -WORLD_LIMIT || v > WORLD_LIMIT - span)
    {
        return None;
    }
    Some(16 * 16 * WORLD_HEIGHT as usize / usize::from(key.y_stride()))
}

/// Native tile layout: x + 16 * (z + 16 * y). Never expand it to 1m voxels.
pub fn valid_distant_tile(key: DistantTileKey, blocks: &[u8]) -> bool {
    distant_tile_block_count(key) == Some(blocks.len()) && blocks.iter().all(|&b| valid_block(b))
}

pub fn distant_plan(center: [i32; 2]) -> DistantPlan {
    if !valid_column(center) {
        return DistantPlan::default();
    }
    let world = center.map(|v| v * 16 + 8);
    let root = world.map(|v| v.div_euclid(512));
    let mut plan = DistantPlan::default();
    let root_radius = FAR_DISTANCE / 512;
    for z in -root_radius..=root_radius {
        for x in -root_radius..=root_radius {
            let key = DistantTileKey {
                origin: [(root[0] + x) * 512, (root[1] + z) * 512],
                stride: 32,
            };
            if distant_tile_block_count(key).is_some() {
                plan.roots.push(key);
            }
        }
    }
    for key in plan.roots.clone() {
        visit(key, center, world, &mut plan);
    }
    debug_assert!(plan.nodes.len() <= MAX_WANTED_TILES);
    plan
}

fn visit(key: DistantTileKey, center: [i32; 2], world: [i32; 2], plan: &mut DistantPlan) {
    plan.nodes.push(key);
    let distance = (0..2)
        .map(|i| {
            (key.origin[i] - world[i])
                .max(world[i] - (key.origin[i] + key.span()))
                .max(0)
        })
        .max()
        .unwrap();
    let guard = (0..2).all(|i| {
        key.origin[i] < (center[i] + FAR_FINE_RADIUS + 1) * 16
            && key.origin[i] + key.span() > (center[i] - FAR_FINE_RADIUS) * 16
    });
    if let Some(children) = key
        .children()
        .filter(|_| guard || distance < 2 * key.span())
    {
        for child in children {
            visit(child, center, world, plan);
        }
    } else {
        plan.leaves.insert(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServerMessage, AIR};

    #[test]
    fn tile_layout_and_protocol_roundtrip() {
        for (stride, size) in [
            (1, 32768),
            (2, 16384),
            (4, 8192),
            (8, 8192),
            (16, 8192),
            (32, 8192),
        ] {
            let key = DistantTileKey {
                origin: [-512, 0],
                stride,
            };
            let blocks = vec![AIR; size];
            assert!(valid_distant_tile(key, &blocks));
            let message = ServerMessage::DistantTile {
                epoch: 7,
                key,
                revision: 3,
                blocks,
            };
            assert_eq!(
                rmp_serde::from_slice::<ServerMessage>(&rmp_serde::to_vec(&message).unwrap())
                    .unwrap(),
                message
            );
        }
        assert!(distant_tile_block_count(DistantTileKey {
            origin: [1, 0],
            stride: 2
        })
        .is_none());
        assert!(distant_tile_block_count(DistantTileKey {
            origin: [0, 0],
            stride: 0
        })
        .is_none());
        assert!(distant_tile_block_count(DistantTileKey {
            origin: [WORLD_LIMIT, 0],
            stride: 32
        })
        .is_none());
    }

    #[test]
    fn plan_bound_guard_negative_seams_and_world_edges() {
        for x in -32..32 {
            for z in -32..32 {
                let center = [x, z];
                let plan = distant_plan(center);
                assert_eq!(plan.roots.len(), 81);
                assert!(plan.nodes.len() <= 801);
                assert_eq!(
                    plan.nodes.iter().copied().collect::<BTreeSet<_>>().len(),
                    plan.nodes.len()
                );
                for dz in -4..=4 {
                    for dx in -4..=4 {
                        assert!(plan.leaves.contains(&DistantTileKey {
                            origin: [(x + dx) * 16, (z + dz) * 16],
                            stride: 1
                        }));
                    }
                }
            }
        }
        for center in [[-2048, -2048], [2047, 2047], [-2048, 2047]] {
            let plan = distant_plan(center);
            assert!(!plan.roots.is_empty());
            assert!(plan
                .nodes
                .iter()
                .all(|&k| distant_tile_block_count(k).is_some()));
        }
        assert!(distant_plan([i32::MAX, 0]).nodes.is_empty());
    }
}
