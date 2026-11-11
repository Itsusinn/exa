//! Authoritative render-only volume tiles. Never insert these into collision Grid.
use exa_protocol::{
    distant_plan, valid_column, valid_distant_tile, DistantTileKey, AIR, MAX_CACHED_TILES,
    MAX_DISTANT_CACHE_BYTES, WORLD_HEIGHT,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Tile {
    pub revision: u64,
    pub mesh_revision: u64,
    pub blocks: Arc<[u8]>,
    last_used: u64,
}

#[derive(Default)]
pub struct DistantCache {
    pub epoch: u64,
    pub center: [i32; 2],
    pub tiles: BTreeMap<DistantTileKey, Tile>,
    pub desired: BTreeSet<DistantTileKey>,
    pub roots: Vec<DistantTileKey>,
    pub leaves: BTreeSet<DistantTileKey>,
    pub dirty: BTreeSet<DistantTileKey>,
    clock: u64,
}

impl DistantCache {
    pub fn view(&mut self, epoch: u64, center: [i32; 2]) -> bool {
        if epoch == 0 || epoch <= self.epoch || !valid_column(center) {
            return false;
        }
        let plan = distant_plan(center);
        self.epoch = epoch;
        self.center = center;
        self.roots = plan.roots;
        let previous = std::mem::replace(&mut self.desired, plan.nodes.into_iter().collect());
        self.leaves = plan.leaves;
        // The host may evict a non-wanted GPU mesh while retaining its raw tile.
        // Re-entering the plan must rebuild it even when the server's source
        // revision is unchanged (and a duplicate payload is correctly ignored).
        for key in self.desired.difference(&previous) {
            if self.tiles.contains_key(key) {
                self.dirty.insert(*key);
            }
        }
        self.clock += 1;
        for (key, tile) in &mut self.tiles {
            if self.desired.contains(key) {
                tile.last_used = self.clock;
            }
        }
        let roots: BTreeSet<_> = self.roots.iter().copied().collect();
        self.tiles.retain(|key, _| {
            roots.contains(&DistantTileKey {
                origin: key.origin.map(|v| v.div_euclid(512) * 512),
                stride: 32,
            })
        });
        // Retain older fine children until their coarser replacement is ready.
        // Closed tile borders have no mesh dependency on the current neighbor LOD.
        self.prune();
        true
    }

    pub fn insert(
        &mut self,
        epoch: u64,
        key: DistantTileKey,
        revision: u64,
        blocks: Vec<u8>,
    ) -> bool {
        if epoch == 0
            || epoch != self.epoch
            || !self.desired.contains(&key)
            || !valid_distant_tile(key, &blocks)
            || self
                .tiles
                .get(&key)
                .is_some_and(|old| revision <= old.revision)
        {
            return false;
        }
        self.clock += 1;
        self.tiles.insert(
            key,
            Tile {
                revision,
                mesh_revision: self.clock,
                blocks: blocks.into(),
                last_used: self.clock,
            },
        );
        self.dirty.insert(key);
        self.prune();
        true
    }

    fn prune(&mut self) {
        // Membership in the current roots is pruned once per view, not by scanning
        // every cached tile against 81 roots for each incoming payload.
        while self.tiles.len() > MAX_CACHED_TILES || self.raw_bytes() > MAX_DISTANT_CACHE_BYTES {
            let oldest = self
                .tiles
                .iter()
                .filter(|(key, _)| !self.desired.contains(*key))
                .min_by_key(|(key, tile)| (tile.last_used, **key))
                .map(|(key, _)| *key);
            // The protocol plan has <=1024 nodes, each <=32768 bytes; all current
            // tiles therefore fit below both hard limits without evicting coverage.
            let Some(oldest) = oldest else {
                break;
            };
            self.tiles.remove(&oldest);
        }
        self.dirty.retain(|key| self.tiles.contains_key(key));
    }

    pub fn raw_bytes(&self) -> usize {
        self.tiles.values().map(|t| t.blocks.len()).sum()
    }

    /// Fine authoritative data may supplement near AO, never near face culling.
    pub fn fine_block(&self, p: [i32; 3]) -> u8 {
        if !(0..WORLD_HEIGHT).contains(&p[1]) {
            return AIR;
        }
        let key = DistantTileKey {
            origin: [p[0].div_euclid(16) * 16, p[2].div_euclid(16) * 16],
            stride: 1,
        };
        if !self.desired.contains(&key) {
            return 255;
        }
        self.tiles.get(&key).map_or(255, |tile| {
            tile.blocks[p[0].rem_euclid(16) as usize
                + 16 * (p[2].rem_euclid(16) as usize + 16 * p[1] as usize)]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exa_protocol::{distant_tile_block_count, STONE};

    #[test]
    fn epochs_bounds_revisions_and_retained_tile_identity() {
        let mut cache = DistantCache::default();
        let key = DistantTileKey {
            origin: [-16, -16],
            stride: 1,
        };
        assert!(!cache.view(0, [0, 0]));
        assert!(cache.view(1, [-1, -1]));
        let data = vec![STONE; distant_tile_block_count(key).unwrap()];
        assert!(!cache.insert(0, key, 1, data.clone()));
        assert!(!cache.insert(1, key, 1, vec![STONE]));
        assert!(cache.insert(1, key, 2, data.clone()));
        assert!(!cache.insert(1, key, 1, data.clone()));
        assert!(!cache.insert(1, key, 2, data));
        let stamp = cache.tiles[&key].mesh_revision;
        assert_eq!(cache.fine_block([-1, 12, -1]), STONE);
        assert!(cache.view(2, [0, -1]));
        assert_eq!(cache.tiles[&key].mesh_revision, stamp);
        assert!(cache.dirty.contains(&key));
        assert!(!cache.view(1, [0, 0]));
        assert!(cache.view(3, [1000, 1000]));
        assert!(cache.tiles.is_empty());
        assert!(cache.dirty.is_empty());
    }

    #[test]
    fn every_current_plan_fits_the_cache_without_dropping_desired_tiles() {
        let mut cache = DistantCache::default();
        for (epoch, center) in [(1, [0, 0]), (2, [17, -19]), (3, [-31, 33])] {
            assert!(cache.view(epoch, center));
            let wanted: Vec<_> = cache.desired.iter().copied().collect();
            for key in wanted {
                if !cache.tiles.contains_key(&key) {
                    assert!(cache.insert(
                        epoch,
                        key,
                        0,
                        vec![AIR; distant_tile_block_count(key).unwrap()]
                    ));
                }
            }
            assert!(cache
                .desired
                .iter()
                .all(|key| cache.tiles.contains_key(key)));
            assert!(cache.tiles.len() <= MAX_CACHED_TILES);
            assert!(cache.raw_bytes() <= MAX_DISTANT_CACHE_BYTES);
        }
    }

    #[test]
    fn returning_to_cached_tile_requeues_geometry_without_a_new_source_revision() {
        let mut cache = DistantCache::default();
        let key = DistantTileKey {
            origin: [0, 0],
            stride: 1,
        };
        cache.view(1, [0, 0]);
        assert!(cache.insert(1, key, 7, vec![STONE; 32768]));
        let stamp = cache.tiles[&key].mesh_revision;
        cache.dirty.remove(&key); // Previously uploaded; host can later evict it.
        cache.view(2, [10, 0]);
        assert!(!cache.desired.contains(&key));
        assert!(cache.tiles.contains_key(&key));
        assert!(!cache.dirty.contains(&key));
        cache.view(3, [0, 0]);
        assert!(cache.desired.contains(&key));
        assert!(cache.dirty.contains(&key));
        assert_eq!(cache.tiles[&key].mesh_revision, stamp);
        assert!(!cache.insert(3, key, 7, vec![STONE; 32768]));
        assert!(cache.dirty.contains(&key));
    }
}
