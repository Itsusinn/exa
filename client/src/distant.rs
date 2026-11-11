//! Bounded CPU-only distant meshing. No engine objects cross the worker boundary.
use crate::mesh::{Mesh, NORMALS};
use exa_client_core::voxel::Client3d;
use exa_protocol::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender},
    Arc,
};

pub const MAX_FINE_VERTICES: usize = 24_576;
pub const MAX_COARSE_VERTICES: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub generation: u64,
    pub epoch: u64,
    pub tile: DistantTileKey,
    pub revision: u64,
    pub mesh_revision: u64,
}

impl Key {
    fn current(self, model: &Client3d, generation: u64) -> bool {
        self.generation == generation
            && model.distant.tiles.get(&self.tile).is_some_and(|c| {
                (c.revision, c.mesh_revision) == (self.revision, self.mesh_revision)
            })
    }
}

struct Job {
    key: Key,
    serial: u64,
    blocks: Arc<[u8]>,
}

pub struct ReadyMesh {
    pub key: Key,
    pub effective_stride: u16,
    pub mesh: Mesh,
}

struct Channels {
    tx: SyncSender<Job>,
    rx: Receiver<Option<ReadyMesh>>,
}

#[derive(Default)]
pub struct Mesher {
    channels: Option<Channels>,
    cancellation: Arc<AtomicU64>,
    active: Option<Key>,
    ready: Option<ReadyMesh>,
    preferred: Option<Key>,
    pub discarded: u64,
    pub error: Option<String>,
}

impl Mesher {
    pub fn reset(&mut self) {
        self.cancellation.fetch_add(1, Ordering::Relaxed);
        self.ready = None;
        self.preferred = None;
        // Keep the active token until its bounded job has acknowledged cancellation;
        // never spawn a second worker during a teleport or reconnect.
    }

    fn start(&mut self) -> bool {
        if self.channels.is_some() {
            return true;
        }
        let (tx, input) = mpsc::sync_channel::<Job>(1);
        let (output, rx) = mpsc::sync_channel(1);
        let cancellation = self.cancellation.clone();
        match std::thread::Builder::new()
            .name("distant-mesh".into())
            .spawn(move || {
                while let Ok(job) = input.recv() {
                    let result = build(&job, &cancellation);
                    if output.send(result).is_err() {
                        break;
                    }
                }
            }) {
            Ok(_) => {
                self.channels = Some(Channels { tx, rx });
                true
            }
            Err(error) => {
                self.error = Some(format!("Distant mesher could not start: {error}"));
                false
            }
        }
    }

    pub fn pump(&mut self, model: &mut Client3d, generation: u64) {
        if self.preferred.is_some_and(|key| {
            !key.current(model, generation)
                || !model.distant.desired.contains(&key.tile)
                || !model.distant.dirty.contains(&key.tile)
        }) {
            self.preferred = None;
        }
        if self
            .ready
            .as_ref()
            .is_some_and(|r| !r.key.current(model, generation))
        {
            self.ready = None;
            self.discarded += 1;
        }
        if let Some(key) = self.active {
            if !key.current(model, generation) {
                self.cancellation.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(channels) = &self.channels {
            match channels.rx.try_recv() {
                Ok(result) => {
                    self.active = None;
                    if let Some(result) = result.filter(|r| r.key.current(model, generation)) {
                        self.ready = Some(result);
                    } else {
                        self.discarded += 1;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.active = None;
                    self.error = Some("Distant mesher stopped unexpectedly".into());
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !model.ready || self.active.is_some() || self.ready.is_some() || self.error.is_some() {
            return;
        }
        let center = model.distant.center;
        let candidate = self.preferred.map(|key| key.tile).or_else(|| {
            model
                .distant
                .dirty
                .iter()
                .filter(|key| model.distant.tiles.contains_key(*key))
                .min_by_key(|c| {
                    let key = **c;
                    let dx = i64::from(key.origin[0] + key.span() / 2 - (center[0] * 16 + 8));
                    let dz = i64::from(key.origin[1] + key.span() / 2 - (center[1] * 16 + 8));
                    // Full-resolution guard first, then spiral outward through available
                    // exact summaries. Historical handoff children are lower priority.
                    (
                        !model.distant.desired.contains(&key),
                        key.stride != 1,
                        key.stride == 1
                            && model
                                .terrain
                                .has_column([key.origin[0] / 16, key.origin[1] / 16]),
                        dx * dx + dz * dz,
                        key,
                    )
                })
                .copied()
        });
        let Some(tile) = candidate else {
            return;
        };
        if !self.start() {
            return;
        }
        let data = &model.distant.tiles[&tile];
        let key = Key {
            generation,
            epoch: model.distant.epoch,
            tile,
            revision: data.revision,
            mesh_revision: data.mesh_revision,
        };
        let job = Job {
            key,
            serial: self.cancellation.load(Ordering::Relaxed),
            blocks: data.blocks.clone(),
        };
        if self.channels.as_ref().unwrap().tx.try_send(job).is_ok() {
            self.active = Some(key);
            self.preferred = None;
        }
    }

    pub fn take(&mut self, model: &mut Client3d, generation: u64) -> Option<ReadyMesh> {
        self.pump(model, generation);
        let mut result = self.ready.take()?;
        if !result.key.current(model, generation) {
            return None;
        }
        // Retained tile jobs survive ordinary view motion; publish in the new view.
        result.key.epoch = model.distant.epoch;
        model.distant.dirty.remove(&result.key.tile);
        Some(result)
    }

    pub fn peek_key(&mut self, model: &mut Client3d, generation: u64) -> Option<Key> {
        self.pump(model, generation);
        self.ready.as_ref().map(|result| result.key)
    }

    /// Let a completed fine mesh yield to a coarse replacement that can free GPU
    /// space. This never consumes the yielded tile's dirty work or adds a result slot.
    pub fn prioritize(
        &mut self,
        model: &mut Client3d,
        generation: u64,
        tile: DistantTileKey,
    ) -> bool {
        self.pump(model, generation);
        let Some(ready) = &self.ready else {
            return false;
        };
        if !model.ready
            || self.error.is_some()
            || self.active.is_some()
            || ready.key.tile == tile
            || !model.distant.desired.contains(&tile)
            || !model.distant.dirty.contains(&tile)
        {
            return false;
        }
        let Some(data) = model.distant.tiles.get(&tile) else {
            return false;
        };
        let key = Key {
            generation,
            epoch: model.distant.epoch,
            tile,
            revision: data.revision,
            mesh_revision: data.mesh_revision,
        };
        let yielded = ready.key.tile;
        if !self.start() {
            return false;
        }
        model.distant.dirty.insert(yielded);
        self.ready = None;
        self.preferred = Some(key);
        self.pump(model, generation);
        true
    }

    pub fn in_flight(&self) -> bool {
        self.active.is_some()
    }
    pub fn ready(&self) -> bool {
        self.ready.is_some()
    }
}

impl Drop for Mesher {
    fn drop(&mut self) {
        self.cancellation.fetch_add(1, Ordering::Relaxed);
        // Dropping both channels wakes an idle receiver or a blocked result sender.
        // No join or disk work on the presentation thread.
    }
}

struct Cells {
    dimensions: [usize; 3],
    step: [usize; 3],
    blocks: Vec<u8>,
}

impl Cells {
    fn from_job(job: &Job, factor: usize) -> Self {
        let edge = 16 / factor;
        let y_stride = usize::from(job.key.tile.y_stride());
        let height = WORLD_HEIGHT as usize / y_stride;
        let horizontal = usize::from(job.key.tile.stride) * factor;
        let mut blocks = vec![AIR; edge * edge * height];
        if factor == 1 {
            blocks.copy_from_slice(&job.blocks);
        } else {
            for y in 0..height {
                for z in 0..edge {
                    for x in 0..edge {
                        let mut counts = [0u16; 256];
                        for dz in 0..factor {
                            for dx in 0..factor {
                                let i = x * factor + dx + 16 * (z * factor + dz + 16 * y);
                                counts[job.blocks[i] as usize] += 1;
                            }
                        }
                        // Exhaustively reduce the native tile cells. No heightmap,
                        // sparse sampling or invented empty source cells.
                        let block = (1..255)
                            .filter(|&b| b != WATER as usize && counts[b] > 0)
                            .max_by_key(|&b| (counts[b], std::cmp::Reverse(b)))
                            .map_or(
                                if counts[WATER as usize] > 0 {
                                    WATER
                                } else {
                                    AIR
                                },
                                |b| b as u8,
                            );
                        blocks[x + edge * (z + edge * y)] = block;
                    }
                }
            }
        }
        Self {
            dimensions: [edge, height, edge],
            step: [horizontal, y_stride, horizontal],
            blocks,
        }
    }

    fn get(&self, p: [i32; 3]) -> u8 {
        if (0..3).any(|a| p[a] < 0 || p[a] >= self.dimensions[a] as i32) {
            return AIR;
        }
        let [x, y, z] = p.map(|v| v as usize);
        self.blocks[x + self.dimensions[0] * (z + self.dimensions[2] * y)]
    }
}

fn build(job: &Job, cancellation: &AtomicU64) -> Option<ReadyMesh> {
    let budget = if job.key.tile.stride == 1 {
        MAX_FINE_VERTICES
    } else {
        MAX_COARSE_VERTICES
    };
    let mut factor = 1;
    loop {
        if cancellation.load(Ordering::Relaxed) != job.serial {
            return None;
        }
        let cells = Cells::from_job(job, factor);
        match mesh(job, &cells, budget, cancellation) {
            Build::Ready(mesh) => {
                return Some(ReadyMesh {
                    key: job.key,
                    effective_stride: u16::from(job.key.tile.stride) * factor as u16,
                    mesh,
                })
            }
            Build::Cancelled => return None,
            Build::OverBudget => {
                // A fully reduced X/Z tile has one cell per native Y layer.
                // Even 64 isolated layers fit the coarse budget; Y never exceeds 4 m.
                assert!(
                    factor < 16,
                    "closed native tile must fit its hard vertex budget"
                );
                factor *= 2;
            }
        }
    }
}

enum Build {
    Ready(Mesh),
    OverBudget,
    Cancelled,
}

fn mesh(job: &Job, cells: &Cells, budget: usize, cancellation: &AtomicU64) -> Build {
    let mut result = Mesh::default();
    let dimensions = cells.dimensions;
    let base = [job.key.tile.origin[0], 0, job.key.tile.origin[1]];
    // Closed boundaries are deliberate: a neighboring tile or parent may not
    // have reached the GPU yet. All visible selections must be disjoint volumes.
    // Opposing internal boundary faces lie inside occupied volumes, not skirts.
    for (face, normal) in NORMALS.iter().enumerate() {
        let axis = face / 2;
        let axes = match axis {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        };
        let width = dimensions[axes[0]];
        let height = dimensions[axes[1]];
        let mut mask = vec![AIR; width * height];
        for plane in 0..dimensions[axis] {
            if cancellation.load(Ordering::Relaxed) != job.serial {
                return Build::Cancelled;
            }
            mask.fill(AIR);
            for v in 0..height {
                for u in 0..width {
                    let mut p = [0i32; 3];
                    p[axis] = plane as i32;
                    p[axes[0]] = u as i32;
                    p[axes[1]] = v as i32;
                    let block = cells.get(p);
                    if block == AIR {
                        continue;
                    }
                    let q = std::array::from_fn(|a| p[a] + normal[a] as i32);
                    let neighbor = cells.get(q);
                    if neighbor == AIR || (neighbor == WATER && block != WATER) {
                        mask[u + width * v] = block;
                    }
                }
            }
            for v in 0..height {
                let mut u = 0;
                while u < width {
                    let block = mask[u + width * v];
                    if block == AIR {
                        u += 1;
                        continue;
                    }
                    let mut w = 1;
                    while u + w < width && mask[u + w + width * v] == block {
                        w += 1;
                    }
                    let mut h = 1;
                    while v + h < height && (u..u + w).all(|x| mask[x + width * (v + h)] == block) {
                        h += 1;
                    }
                    if result.vertices.len() + 6 > budget {
                        return Build::OverBudget;
                    }
                    let mut min = base.map(|v| v as f32);
                    min[axis] += (plane * cells.step[axis]) as f32;
                    min[axes[0]] += (u * cells.step[axes[0]]) as f32;
                    min[axes[1]] += (v * cells.step[axes[1]]) as f32;
                    let mut size = cells.step.map(|v| v as f32);
                    size[axes[0]] *= w as f32;
                    size[axes[1]] *= h as f32;
                    let (color, pattern) = crate::presentation::terrain_finish(block, face);
                    result.block_face(min, size, face, color, pattern);
                    for row in v..v + h {
                        mask[u + width * row..u + w + width * row].fill(AIR);
                    }
                    u += w;
                }
            }
        }
    }
    Build::Ready(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(origin: [i32; 2], stride: u8, blocks: Vec<u8>) -> Job {
        Job {
            key: Key {
                generation: 1,
                epoch: 1,
                tile: DistantTileKey { origin, stride },
                revision: 1,
                mesh_revision: 1,
            },
            serial: 0,
            blocks: blocks.into(),
        }
    }

    #[test]
    fn native_volume_mesh_preserves_floating_slabs_and_signed_world_alignment() {
        for stride in [1, 2, 4, 8, 16, 32] {
            let tile = DistantTileKey {
                origin: [-512, -1024],
                stride,
            };
            let mut blocks = vec![AIR; distant_tile_block_count(tile).unwrap()];
            for z in 0..16 {
                for x in 0..16 {
                    blocks[x + 16 * (z + 16 * 3)] = PLANK;
                }
            }
            let job = job(tile.origin, stride, blocks);
            let cells = Cells::from_job(&job, 1);
            assert_eq!(
                cells.dimensions,
                [16, WORLD_HEIGHT as usize / tile.y_stride() as usize, 16]
            );
            assert!(
                cells.blocks.len() <= 32768,
                "never expand a 512 m tile into metre cells"
            );
            let result = build(&job, &AtomicU64::new(0)).unwrap();
            assert_eq!(result.effective_stride, u16::from(stride));
            assert_eq!(result.mesh.vertices.len(), 36);
            assert!(result.mesh.vertices.iter().all(|v| (tile.origin[0] as f32
                ..=(tile.origin[0] + tile.span()) as f32)
                .contains(&v.position[0])
                && (tile.origin[1] as f32..=(tile.origin[1] + tile.span()) as f32)
                    .contains(&v.position[2])
                && (3.0 * tile.y_stride() as f32..=4.0 * tile.y_stride() as f32)
                    .contains(&v.position[1])));
        }
    }

    #[test]
    fn mixed_resolution_tiles_keep_closed_shared_boundaries() {
        let mut fine = vec![AIR; 32768];
        fine[15 + 16 * (4 + 16 * 6)] = STONE;
        let fine = build(&job([-16, 0], 1, fine), &AtomicU64::new(0)).unwrap();
        let coarse_key = DistantTileKey {
            origin: [0, 0],
            stride: 2,
        };
        let mut coarse = vec![AIR; distant_tile_block_count(coarse_key).unwrap()];
        coarse[16 * (2 + 16 * 3)] = STONE;
        let coarse = build(&job([0, 0], 2, coarse), &AtomicU64::new(0)).unwrap();
        assert_eq!(fine.mesh.vertices.len(), 36);
        assert_eq!(coarse.mesh.vertices.len(), 36);
        assert!(fine
            .mesh
            .vertices
            .iter()
            .filter(|v| v.normal == [1.0, 0.0, 0.0])
            .all(|v| v.position[0] == 0.0));
        assert!(coarse
            .mesh
            .vertices
            .iter()
            .filter(|v| v.normal == [-1.0, 0.0, 0.0])
            .all(|v| v.position[0] == 0.0));
    }

    #[test]
    fn checkerboards_degrade_with_bounded_vertices_without_coarsening_y() {
        for stride in [1, 2, 32] {
            let tile = DistantTileKey {
                origin: [0, 0],
                stride,
            };
            let mut blocks = vec![AIR; distant_tile_block_count(tile).unwrap()];
            for y in 0..WORLD_HEIGHT as usize / tile.y_stride() as usize {
                for z in 0..16 {
                    for x in 0..16 {
                        if (x + y + z) % 2 == 0 {
                            blocks[x + 16 * (z + 16 * y)] = PLANK;
                        }
                    }
                }
            }
            let job = job([0, 0], stride, blocks);
            let result = build(&job, &AtomicU64::new(0)).unwrap();
            assert!(result.effective_stride > u16::from(stride));
            assert!(!result.mesh.vertices.is_empty());
            assert!(
                result.mesh.vertices.len()
                    <= if stride == 1 {
                        MAX_FINE_VERTICES
                    } else {
                        MAX_COARSE_VERTICES
                    }
            );
            assert_eq!(Cells::from_job(&job, 16).step[1], tile.y_stride() as usize);
        }
    }

    #[test]
    fn retained_results_survive_view_motion_but_not_edits_eviction_or_generation() {
        let mut model = Client3d::default();
        let tile = DistantTileKey {
            origin: [0, 0],
            stride: 1,
        };
        model.distant.view(1, [0, 0]);
        model.distant.insert(1, tile, 1, vec![STONE; 32768]);
        let mut job = job([0, 0], 1, vec![STONE; 32768]);
        job.key.mesh_revision = model.distant.tiles[&tile].mesh_revision;
        assert!(build(&job, &AtomicU64::new(1)).is_none());
        let mut mesher = Mesher::default();
        mesher.ready = build(&job, &AtomicU64::new(0));
        model.distant.view(2, [1, 0]);
        assert_eq!(mesher.take(&mut model, 1).unwrap().key.epoch, 2);
        assert!(!model.distant.dirty.contains(&tile));
        mesher.ready = build(&job, &AtomicU64::new(0));
        model.distant.insert(2, tile, 2, vec![AIR; 32768]);
        assert!(mesher.take(&mut model, 1).is_none());
        assert!(model.distant.dirty.contains(&tile));
        mesher.ready = build(&job, &AtomicU64::new(0));
        assert!(mesher.take(&mut model, 2).is_none());
        model.distant.view(3, [1000, 1000]);
        assert!(model.distant.tiles.is_empty());
        assert!(model.terrain.solid([0, 5, 0]));
    }

    #[test]
    fn empty_tile_is_a_completed_zero_geometry_result() {
        let result = build(&job([0, 0], 1, vec![AIR; 32768]), &AtomicU64::new(0)).unwrap();
        assert!(result.mesh.vertices.is_empty());
        assert_eq!(result.effective_stride, 1);
    }

    #[test]
    fn peeking_ready_key_does_not_consume_result_or_clear_dirty_work() {
        let mut model = Client3d::default();
        let tile = DistantTileKey {
            origin: [0, 0],
            stride: 1,
        };
        model.distant.view(1, [0, 0]);
        model.distant.insert(1, tile, 1, vec![AIR; 32768]);
        let mut job = job([0, 0], 1, vec![AIR; 32768]);
        job.key.mesh_revision = model.distant.tiles[&tile].mesh_revision;
        let mut mesher = Mesher::default();
        mesher.ready = build(&job, &AtomicU64::new(0));
        for _ in 0..2 {
            assert_eq!(mesher.peek_key(&mut model, 1).unwrap().tile, tile);
            assert!(mesher.ready());
            assert!(model.distant.dirty.contains(&tile));
        }
        assert_eq!(mesher.take(&mut model, 1).unwrap().key.tile, tile);
        assert!(!mesher.ready());
        assert!(!model.distant.dirty.contains(&tile));
    }

    fn wait_for_mesh(mesher: &mut Mesher, model: &mut Client3d) -> ReadyMesh {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(result) = mesher.take(model, 1) {
                return result;
            }
            assert!(mesher.error.is_none());
            assert!(
                std::time::Instant::now() < deadline,
                "bounded mesh worker did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn ready_fine_mesh_yields_to_coarsening_then_is_generated_again() {
        let mut model = Client3d::default();
        model.distant.view(1, [0, 0]);
        model.ready = true;
        let fine = DistantTileKey {
            origin: [0, 0],
            stride: 1,
        };
        let coarse = model.distant.roots[0];
        assert!(model.distant.leaves.contains(&coarse));
        model.distant.insert(
            1,
            fine,
            1,
            vec![AIR; distant_tile_block_count(fine).unwrap()],
        );
        model.distant.insert(
            1,
            coarse,
            1,
            vec![AIR; distant_tile_block_count(coarse).unwrap()],
        );
        let mut mesher = Mesher::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(key) = mesher.peek_key(&mut model, 1) {
                assert_eq!(key.tile, fine);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fine mesh did not become ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let missing = model.distant.roots[1];
        assert!(!mesher.prioritize(&mut model, 1, missing));
        assert_eq!(mesher.peek_key(&mut model, 1).unwrap().tile, fine);
        assert!(mesher.prioritize(&mut model, 1, coarse));
        assert!(model.distant.dirty.contains(&fine));
        assert_eq!(wait_for_mesh(&mut mesher, &mut model).key.tile, coarse);
        assert!(model.distant.dirty.contains(&fine));
        assert!(!model.distant.dirty.contains(&coarse));
        assert_eq!(wait_for_mesh(&mut mesher, &mut model).key.tile, fine);
        assert!(!model.distant.dirty.contains(&fine));
    }

    #[test]
    fn preferred_work_survives_retained_view_but_is_cleared_by_reset_or_generation() {
        let mut model = Client3d::default();
        model.distant.view(1, [0, 0]);
        let tile = DistantTileKey {
            origin: [0, 0],
            stride: 1,
        };
        model.distant.insert(1, tile, 1, vec![AIR; 32768]);
        let key = Key {
            generation: 1,
            epoch: 1,
            tile,
            revision: 1,
            mesh_revision: model.distant.tiles[&tile].mesh_revision,
        };
        let mut mesher = Mesher::default();
        mesher.preferred = Some(key);
        model.distant.view(2, [1, 0]);
        mesher.pump(&mut model, 1); // Not ready: validate without submitting a job.
        assert_eq!(mesher.preferred, Some(key));
        mesher.pump(&mut model, 2);
        assert!(mesher.preferred.is_none());
        mesher.preferred = Some(key);
        mesher.reset();
        assert!(mesher.preferred.is_none());
        assert!(model.distant.dirty.contains(&tile));
    }
}
