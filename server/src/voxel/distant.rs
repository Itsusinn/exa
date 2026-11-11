//! Exact, progressive display-only terrain. Preview data never enters Grid/save.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[path = "distant_source.rs"]
mod source;
#[path = "distant_tile.rs"]
mod tile;
use source::Summary;
use tile::Assembly;

pub(crate) const BATCH_COLUMNS: usize = 4;
/// At most two 32KiB fine tiles can wait behind the active network frame.
pub(crate) const FAR_QUEUE_CAPACITY: usize = 2;
const SOURCE_ENTRIES: usize = 131072;
const SOURCE_BYTES: usize = 32 * 1024 * 1024;
const TILE_ENTRIES: usize = 2048;
const TILE_BYTES: usize = 32 * 1024 * 1024;
const CACHE_PATCHES: usize = 32;
const QUEUE_RESERVE: usize = SECTIONS_PER_COLUMN + 4;
const SEND_BYTES: usize = 64 * 1024;
type Key = DistantTileKey;

#[derive(Default)]
pub(super) struct View {
    center: Option<[i32; 2]>,
    epoch: u64,
    nodes: Vec<Key>,
    sent: BTreeMap<Key, u64>,
}

struct CachedSource {
    revision: u64,
    known: bool,
    used: u64,
    summary: Arc<Summary>,
}
struct CachedTile {
    revision: u64,
    used: u64,
    assembly: Assembly,
}

#[derive(Default)]
pub(super) struct State {
    sources: BTreeMap<[i32; 2], CachedSource>,
    source_lru: BTreeSet<(u64, [i32; 2])>,
    source_bytes: usize,
    tiles: BTreeMap<Key, CachedTile>,
    tile_lru: BTreeSet<(u64, Key)>,
    tile_bytes: usize,
    revisions: BTreeMap<[i32; 2], u64>,
    tile_revisions: BTreeMap<Key, u64>,
    revision: u64,
    clock: u64,
    turn: usize,
    views: Vec<(u32, [i32; 2])>,
    wanted: BTreeSet<Key>,
}

fn key_for(column: [i32; 2], stride: u8) -> Key {
    let span = 16 * i32::from(stride);
    Key {
        origin: column.map(|v| (v * 16).div_euclid(span) * span),
        stride,
    }
}

fn priority(key: Key, center: [i32; 2]) -> (u8, i32, u8, Key) {
    let guard = key.stride == 1
        && (0..2)
            .all(|i| key.origin[i].div_euclid(16).abs_diff(center[i]) <= FAR_FINE_RADIUS as u32);
    let p = center.map(|v| v * 16 + 8);
    let distance = (0..2)
        .map(|i| {
            (key.origin[i] - p[i])
                .max(p[i] - (key.origin[i] + key.span()))
                .max(0)
        })
        .max()
        .unwrap();
    (u8::from(!guard), distance + key.span() / 2, key.stride, key)
}

impl State {
    fn source_revision(&self, c: [i32; 2]) -> u64 {
        self.revisions.get(&c).copied().unwrap_or(0)
    }
    fn tile_revision(&self, k: Key) -> u64 {
        self.tile_revisions.get(&k).copied().unwrap_or(0)
    }
    fn minimum(&self, c: [i32; 2]) -> Option<u8> {
        [1, 2, 4, 8, 16, 32]
            .into_iter()
            .find(|&s| self.wanted.contains(&key_for(c, s)))
            .map(|s| s.min(16))
    }
    fn next_clock(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    pub(super) fn invalidate(&mut self, c: [i32; 2]) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("terrain revision exhausted");
        self.revisions.insert(c, self.revision);
        if let Some(old) = self.sources.remove(&c) {
            self.source_bytes -= old.summary.bytes();
            self.source_lru.remove(&(old.used, c));
        }
        for stride in [1, 2, 4, 8, 16, 32] {
            let key = key_for(c, stride);
            self.tile_revisions.insert(key, self.revision);
            if let Some(tile) = self.tiles.get_mut(&key) {
                let before = tile.assembly.bytes();
                tile.assembly.clear(c);
                tile.revision = self.revision;
                self.tile_bytes -= before - tile.assembly.bytes();
            }
        }
    }

    fn trim_sources(&mut self) {
        while self.sources.len() > SOURCE_ENTRIES || self.source_bytes > SOURCE_BYTES {
            let Some((_, c)) = self.source_lru.pop_first() else {
                break;
            };
            self.source_bytes -= self.sources.remove(&c).unwrap().summary.bytes();
        }
    }
    fn trim_tiles(&mut self) {
        while self.tiles.len() > TILE_ENTRIES || self.tile_bytes > TILE_BYTES {
            let Some((_, key)) = self.tile_lru.pop_first() else {
                break;
            };
            self.tile_bytes -= self.tiles.remove(&key).unwrap().assembly.bytes();
        }
    }
    fn touch_tile(&mut self, key: Key) {
        let used = self.next_clock();
        if let Some(tile) = self.tiles.get_mut(&key) {
            self.tile_lru.remove(&(tile.used, key));
            tile.used = used;
            self.tile_lru.insert((used, key));
        }
    }
    fn ensure_tile(&mut self, key: Key) {
        if self.tiles.contains_key(&key) {
            return;
        }
        let assembly = Assembly::new(key);
        let used = self.next_clock();
        self.tile_bytes += assembly.bytes();
        self.tiles.insert(
            key,
            CachedTile {
                revision: self.tile_revision(key),
                used,
                assembly,
            },
        );
        self.tile_lru.insert((used, key));
        self.trim_tiles();
    }
    fn feed(&mut self, c: [i32; 2], summary: &Summary) {
        // One fully read/generated source contributes to every requested level.
        for stride in [1, 2, 4, 8, 16, 32] {
            let key = key_for(c, stride);
            if !self.wanted.contains(&key) || summary.get(stride.min(16)).is_none() {
                continue;
            }
            self.ensure_tile(key);
            if let Some(tile) = self.tiles.get_mut(&key) {
                let before = tile.assembly.bytes();
                tile.assembly.add(c, summary);
                self.tile_bytes += tile.assembly.bytes() - before;
            }
            self.touch_tile(key);
            self.trim_tiles();
        }
    }
    fn cache_source(&mut self, output: Output) {
        let c = output.column;
        if let Some(old) = self.sources.remove(&c) {
            self.source_bytes -= old.summary.bytes();
            self.source_lru.remove(&(old.used, c));
        }
        let summary = Arc::new(output.summary);
        let used = self.next_clock();
        self.source_bytes += summary.bytes();
        self.sources.insert(
            c,
            CachedSource {
                revision: output.revision,
                known: output.known,
                used,
                summary: Arc::clone(&summary),
            },
        );
        self.source_lru.insert((used, c));
        self.trim_sources();
        self.feed(c, &summary);
    }

    fn request(
        &mut self,
        nodes: &[Key],
        sent: &BTreeMap<Key, u64>,
        terrain: &Grid,
        selected: &BTreeSet<[i32; 2]>,
        patches: &mut usize,
    ) -> Option<([i32; 2], u8)> {
        for &key in nodes {
            if sent.get(&key) == Some(&self.tile_revision(key)) {
                continue;
            }
            self.ensure_tile(key);
            if self
                .tiles
                .get(&key)
                .is_some_and(|t| t.assembly.blocks().is_some())
            {
                continue;
            }
            let n = i32::from(key.stride);
            let start = key.origin.map(|v| v.div_euclid(16));
            for z in 0..n {
                for x in 0..n {
                    let c = [start[0] + x, start[1] + z];
                    if selected.contains(&c)
                        || self.tiles.get(&key).is_some_and(|t| t.assembly.has(c))
                    {
                        continue;
                    }
                    let minimum = self.minimum(c).unwrap_or(key.stride.min(16));
                    let cached = self
                        .sources
                        .get(&c)
                        .filter(|s| {
                            s.revision == self.source_revision(c)
                                && s.known == terrain.has_column(c)
                                && s.summary.get(minimum).is_some()
                        })
                        .map(|s| Arc::clone(&s.summary));
                    if let Some(summary) = cached {
                        if *patches == 0 {
                            return None;
                        }
                        *patches -= 1;
                        let used = self.next_clock();
                        let old = self.sources.get_mut(&c).unwrap();
                        self.source_lru.remove(&(old.used, c));
                        old.used = used;
                        self.source_lru.insert((used, c));
                        self.feed(c, &summary);
                    } else {
                        return Some((c, minimum));
                    }
                }
            }
        }
        None
    }
}

pub(crate) struct Input {
    column: [i32; 2],
    minimum: u8,
    revision: u64,
    seed: u64,
    blocks: Option<Vec<u8>>,
}
pub(crate) struct Output {
    column: [i32; 2],
    revision: u64,
    known: bool,
    summary: Summary,
}

pub(crate) fn build_batch(inputs: Vec<Input>, cancelled: &AtomicBool) -> Vec<Output> {
    inputs
        .into_iter()
        .take(BATCH_COLUMNS)
        .map_while(|input| {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let known = input.blocks.is_some();
            let blocks = match input.blocks {
                Some(blocks) => Ok(blocks),
                None => terrain::build_column(input.seed, input.column),
            };
            Some(blocks.map(|blocks| Output {
                column: input.column,
                revision: input.revision,
                known,
                summary: Summary::from_column(blocks, input.minimum),
            }))
        })
        .filter_map(|result| match result {
            Ok(output) => Some(output),
            Err(error) => {
                tracing::error!(%error,"distant preview failed; facts unchanged");
                None
            }
        })
        .collect()
}

impl VoxelWorld {
    pub(crate) fn attach_distant_sender(
        &mut self,
        id: u32,
        tx: mpsc::Sender<ServerMessage>,
    ) -> Result<()> {
        let player = self.players.get_mut(&id).context("unknown player")?;
        anyhow::ensure!(
            player.distant_tx.is_none(),
            "distant sender already attached"
        );
        player.distant_tx = Some(tx);
        Ok(())
    }

    pub(crate) fn stream_distant(&mut self) {
        for player in self.players.values_mut() {
            let Some(tx) = player.distant_tx.as_ref() else {
                continue;
            };
            let center = column_at(player.body.position);
            if player.tx.capacity() <= QUEUE_RESERVE || tx.capacity() == 0 {
                continue;
            }
            if player.distant.center != Some(center) {
                let epoch = player
                    .distant
                    .epoch
                    .checked_add(1)
                    .expect("view epoch exhausted");
                if tx
                    .try_send(ServerMessage::DistantView { epoch, center })
                    .is_err()
                {
                    continue;
                }
                let plan = distant_plan(center);
                let allowed: BTreeSet<_> = plan.nodes.iter().copied().collect();
                player.distant.center = Some(center);
                player.distant.epoch = epoch;
                player.distant.nodes = plan.nodes;
                player.distant.nodes.sort_by_key(|&k| priority(k, center));
                player.distant.sent.retain(|k, _| allowed.contains(k));
            }
            let (mut count, mut bytes) = (0, 0);
            for &key in &player.distant.nodes {
                if count == BATCH_COLUMNS
                    || player.tx.capacity() <= QUEUE_RESERVE
                    || tx.capacity() == 0
                {
                    break;
                }
                let revision = self.distant.tile_revision(key);
                if player.distant.sent.get(&key) == Some(&revision) {
                    continue;
                }
                let Some(tile) = self.distant.tiles.get(&key) else {
                    continue;
                };
                let Some(blocks) = tile.assembly.blocks() else {
                    continue;
                };
                if tile.revision != revision || bytes + blocks.len() > SEND_BYTES {
                    continue;
                }
                let size = blocks.len();
                if tx
                    .try_send(ServerMessage::DistantTile {
                        epoch: player.distant.epoch,
                        key,
                        revision,
                        blocks: blocks.to_vec(),
                    })
                    .is_err()
                {
                    break;
                }
                count += 1;
                bytes += size;
                player.distant.sent.insert(key, revision);
                self.distant.touch_tile(key);
            }
        }
        let mut views: Vec<_> = self
            .players
            .iter()
            .filter_map(|(&id, p)| p.distant.center.map(|c| (id, c)))
            .collect();
        views.sort_unstable();
        if views != self.distant.views {
            self.distant.views = views;
            self.distant.wanted = self
                .players
                .values()
                .flat_map(|p| p.distant.nodes.iter().copied())
                .collect();
        }
    }

    pub(crate) fn distant_input(&mut self) -> Vec<Input> {
        let mut players: Vec<_> = self.players.iter().collect();
        players.sort_by_key(|(id, _)| **id);
        if players.is_empty() {
            return vec![];
        }
        let start = self.distant.turn % players.len();
        players.rotate_left(start);
        self.distant.turn = self.distant.turn.wrapping_add(1);
        let mut requests = Vec::new();
        let mut selected = BTreeSet::new();
        let mut patches = CACHE_PATCHES;
        for _ in 0..BATCH_COLUMNS {
            for (_, p) in &players {
                if requests.len() == BATCH_COLUMNS {
                    break;
                }
                if p.tx.capacity() <= QUEUE_RESERVE
                    || p.distant_tx.as_ref().is_none_or(|tx| tx.capacity() == 0)
                    || p.distant.center != Some(column_at(p.body.position))
                {
                    continue;
                }
                if let Some((column, minimum)) = self.distant.request(
                    &p.distant.nodes,
                    &p.distant.sent,
                    &self.terrain,
                    &selected,
                    &mut patches,
                ) {
                    selected.insert(column);
                    requests.push((column, minimum));
                }
            }
        }
        requests
            .into_iter()
            .map(|(column, minimum)| {
                let blocks = self.terrain.has_column(column).then(|| {
                    let mut blocks = Vec::with_capacity(32768);
                    for y in (0..WORLD_HEIGHT).step_by(16) {
                        blocks.extend_from_slice(
                            &self.terrain.blocks[&[column[0] * 16, y, column[1] * 16]],
                        );
                    }
                    blocks
                });
                Input {
                    column,
                    minimum,
                    revision: self.distant.source_revision(column),
                    seed: self.world_seed,
                    blocks,
                }
            })
            .collect()
    }

    pub(crate) fn apply_distant(&mut self, outputs: Vec<Output>) {
        for output in outputs.into_iter().take(BATCH_COLUMNS) {
            if output.revision != self.distant.source_revision(output.column)
                || output.known != self.terrain.has_column(output.column)
                || self.distant.minimum(output.column).is_none()
            {
                continue;
            }
            self.distant.cache_source(output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Observer {
        live: mpsc::Receiver<ServerMessage>,
        far: mpsc::Receiver<ServerMessage>,
    }
    impl Observer {
        fn try_recv(&mut self) -> std::result::Result<ServerMessage, mpsc::error::TryRecvError> {
            self.far.try_recv()
        }
    }

    fn observer(world: &mut VoxelWorld, id: u32, center: [i32; 2], capacity: usize) -> Observer {
        let (tx, live) = mpsc::channel(capacity);
        world.register_player(id, tx).unwrap();
        let (far_tx, mut far) = mpsc::channel(FAR_QUEUE_CAPACITY);
        world.attach_distant_sender(id, far_tx).unwrap();
        world.players.get_mut(&id).unwrap().body.position = [
            center[0] as f32 * 16.0 + 0.5,
            110.0,
            center[1] as f32 * 16.0 + 0.5,
        ];
        world.stream_distant();
        while far.try_recv().is_ok() {}
        Observer { live, far }
    }
    fn single(world: &mut VoxelWorld, c: [i32; 2]) -> Vec<Input> {
        let key = key_for(c, 1);
        world.players.get_mut(&1).unwrap().distant.nodes = vec![key];
        world.distant.wanted = [1, 2, 4, 8, 16, 32]
            .into_iter()
            .map(|s| key_for(c, s))
            .collect();
        world.distant_input()
    }
    fn save_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/distant-tests");
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let path = dir.path().join("world3d.bin");
        (dir, path)
    }

    #[test]
    fn unknown_exact_preview_does_not_explore_or_save_and_child_can_arrive_first() {
        let mut world = VoxelWorld::fixture(17);
        let mut rx = observer(&mut world, 1, [-20, 10], 64);
        let c = [-24, 10];
        let before = world.terrain.clone();
        let inputs = single(&mut world, c);
        assert_eq!(inputs.len(), 1);
        assert!(inputs[0].blocks.is_none());
        let outputs = build_batch(inputs, &AtomicBool::new(false));
        assert_eq!(
            outputs[0].summary.get(1).unwrap(),
            terrain::build_column(17, c).unwrap()
        );
        world.apply_distant(outputs);
        world.stream_distant();
        assert!(
            matches!(rx.try_recv().unwrap(),ServerMessage::DistantTile{key,..} if key==key_for(c,1))
        );
        assert!(world.distant.tiles[&key_for(c, 32)]
            .assembly
            .blocks()
            .is_none());
        assert_eq!(world.terrain, before);
        assert!(world.dirty_chunks.is_empty());
        let (_dir, path) = save_path();
        world.save(&path).unwrap();
        let restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.terrain, before);
        assert!(!restored.terrain.has_column(c));
    }

    #[test]
    fn saved_facts_win_and_edits_invalidate_every_ancestor_without_rebuilding_others() {
        let mut world = VoxelWorld::fixture(17);
        let c = [-2, -1];
        let position = [-17, 127, -1];
        terrain::ensure_column(&mut world.terrain, 17, c).unwrap();
        world.terrain.set(position, PLANK);
        let (_dir, path) = save_path();
        world.save(&path).unwrap();
        let mut world = VoxelWorld::load(&path).unwrap();
        let _rx = observer(&mut world, 1, [2, -1], 64);
        let inputs = single(&mut world, c);
        let outputs = build_batch(inputs, &AtomicBool::new(false));
        assert_eq!(outputs[0].summary.get(1).unwrap()[32767], PLANK);
        world.apply_distant(outputs);
        for stride in [1, 2, 4, 8, 16, 32] {
            assert!(world.distant.tiles[&key_for(c, stride)].assembly.has(c));
        }
        world.players.get_mut(&1).unwrap().distant.sent.clear();
        let stale = Input {
            column: c,
            minimum: 1,
            revision: 0,
            seed: 17,
            blocks: Some(vec![AIR; 32768]),
        };
        world.terrain.set(position, STONE);
        world.distant.invalidate(c);
        world.apply_distant(build_batch(vec![stale], &AtomicBool::new(false)));
        for stride in [1, 2, 4, 8, 16, 32] {
            assert!(!world.distant.tiles[&key_for(c, stride)].assembly.has(c));
        }
        let inputs = single(&mut world, c);
        world.apply_distant(build_batch(inputs, &AtomicBool::new(false)));
        assert_eq!(
            world.distant.tiles[&key_for(c, 1)]
                .assembly
                .blocks()
                .unwrap()[32767],
            STONE
        );
    }

    #[test]
    fn teleport_generation_invalidates_sent_preview_and_ancestors_only_once() {
        let mut world = VoxelWorld::fixture(17);
        let mut rx = observer(&mut world, 1, [-20, 10], 64);
        let column = [-24, 10];
        let child = key_for(column, 1);
        let parent = key_for(column, 2);
        let columns = [[-24, 10], [-23, 10], [-24, 11], [-23, 11]];
        world.players.get_mut(&1).unwrap().distant.nodes = vec![child, parent];
        world.distant.wanted = columns
            .into_iter()
            .flat_map(|c| [1, 2, 4, 8, 16, 32].into_iter().map(move |s| key_for(c, s)))
            .collect();
        let outputs = build_batch(
            columns
                .into_iter()
                .map(|c| Input {
                    column: c,
                    minimum: 1,
                    revision: 0,
                    seed: 17,
                    blocks: None,
                })
                .collect(),
            &AtomicBool::new(false),
        );
        let ray = outputs[0].summary.get(1).unwrap();
        let y = (1..125)
            .rev()
            .find(|&y| (y..y + 3).all(|h| ray[256 * h] == AIR))
            .unwrap();
        world.apply_distant(outputs);
        world.stream_distant();
        let mut sent = BTreeSet::new();
        while let Ok(message) = rx.try_recv() {
            if let ServerMessage::DistantTile { key, revision, .. } = message {
                assert_eq!(revision, 0);
                sent.insert(key);
            }
        }
        assert!(sent.contains(&child) && sent.contains(&parent));
        let command = format!("/tp -383.5 {y} 160.5");
        world.chat(1, command.clone()).unwrap();
        assert_eq!(column_at(world.players[&1].body.position), column);
        assert!(world.terrain.has_column(column));
        let revision = world.distant.source_revision(column);
        assert!(revision > 0);
        for stride in [1, 2, 4, 8, 16, 32] {
            let key = key_for(column, stride);
            assert!(world.distant.tile_revision(key) > 0);
            assert!(!world.distant.tiles[&key].assembly.has(column));
        }
        world.tick += 30;
        let revisions = world.distant.tile_revisions.clone();
        world.chat(1, command).unwrap();
        assert_eq!(world.distant.source_revision(column), revision);
        assert_eq!(world.distant.tile_revisions, revisions);
    }

    #[test]
    fn source_cache_is_reused_across_levels_and_empty_is_a_complete_result() {
        let mut world = VoxelWorld::fixture(17);
        let c = [0, 0];
        let _rx = observer(&mut world, 1, [0, 0], 64);
        world.distant.wanted = [1, 2, 4, 8, 16, 32]
            .into_iter()
            .map(|s| key_for(c, s))
            .collect();
        world.distant.cache_source(Output {
            column: c,
            revision: 0,
            known: true,
            summary: Summary::from_column(vec![AIR; 32768], 1),
        });
        assert!(world.distant.tiles[&key_for(c, 1)]
            .assembly
            .blocks()
            .unwrap()
            .iter()
            .all(|&b| b == AIR));
        // Evict one derived tile; the full source is not generated/read again.
        let key = key_for(c, 1);
        let removed = world.distant.tiles.remove(&key).unwrap();
        world.distant.tile_bytes -= removed.assembly.bytes();
        world.distant.tile_lru.remove(&(removed.used, key));
        world.players.get_mut(&1).unwrap().distant.nodes = vec![key];
        assert!(world.distant_input().is_empty());
        assert!(world.distant.tiles[&key].assembly.blocks().is_some());
    }

    #[test]
    fn source_tokens_reject_exploration_change_but_allow_current_old_view_result() {
        let mut world = VoxelWorld::fixture(17);
        let c = [-24, 10];
        let _rx = observer(&mut world, 1, [-20, 10], 64);
        let inputs = single(&mut world, c);
        let outputs = build_batch(inputs, &AtomicBool::new(false));
        terrain::ensure_column(&mut world.terrain, 17, c).unwrap();
        world.apply_distant(outputs);
        assert!(world.distant.sources.is_empty());
        let inputs = single(&mut world, c);
        let outputs = build_batch(inputs, &AtomicBool::new(false));
        world.players.get_mut(&1).unwrap().body.position[0] += 16.0;
        world.stream_distant();
        world.apply_distant(outputs);
        assert!(world.distant.sources.contains_key(&c));
        let stale = Input {
            column: c,
            minimum: 1,
            revision: 0,
            seed: 17,
            blocks: Some(vec![AIR; 32768]),
        };
        world.players.get_mut(&1).unwrap().body.position = [20000.0, 110.0, 20000.0];
        world.stream_distant();
        world.distant.sources.clear();
        world.distant.source_lru.clear();
        world.distant.source_bytes = 0;
        world.apply_distant(build_batch(vec![stale], &AtomicBool::new(false)));
        assert!(world.distant.sources.is_empty());
    }

    #[test]
    fn near_stream_has_priority_and_worker_selection_is_fair_bounded_cancellable() {
        let mut world = VoxelWorld::fixture(17);
        let (tx, mut rx) = mpsc::channel(QUEUE_RESERVE);
        world.register_player(1, tx).unwrap();
        let (far_tx, mut far_rx) = mpsc::channel(FAR_QUEUE_CAPACITY);
        world.attach_distant_sender(1, far_tx).unwrap();
        world.stream_chunks();
        world.stream_distant();
        assert!(world.players[&1].distant.center.is_none());
        assert!(world.distant_input().is_empty());
        assert!(matches!(
            far_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        let mut count = 0;
        while let Ok(m) = rx.try_recv() {
            assert!(matches!(m, ServerMessage::Chunk3d { .. }));
            count += 1;
        }
        assert_eq!(count, 8);
        let mut world = VoxelWorld::fixture(17);
        let _receivers: Vec<_> = (1..=3)
            .map(|id| observer(&mut world, id, [id as i32 * 400, 20], 64))
            .collect();
        for _ in 0..3 {
            let inputs = world.distant_input();
            assert_eq!(inputs.len(), 4);
            for id in 1..=3 {
                assert!(inputs.iter().any(|i| i.column[0].abs_diff(id * 400) <= 4));
            }
            assert!(build_batch(inputs, &AtomicBool::new(true)).is_empty());
        }
    }

    #[test]
    fn far_backlog_cannot_consume_live_slots_and_stops_preview_work() {
        let mut world = VoxelWorld::fixture(17);
        let mut rx = observer(&mut world, 1, [0, 0], 64);
        for x in 0..4 {
            world.distant.cache_source(Output {
                column: [x, 0],
                revision: 0,
                known: world.terrain.has_column([x, 0]),
                summary: Summary::from_column(vec![AIR; 32768], 1),
            });
        }
        world.stream_distant();
        assert_eq!(world.players[&1].tx.capacity(), 64);
        assert_eq!(world.players[&1].distant_tx.as_ref().unwrap().capacity(), 0);
        assert!(world.distant_input().is_empty());
        assert!(matches!(
            rx.live.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let (mut snapshots, mut chunks) = (0, 0);
        for _ in 0..8 {
            world.tick();
            world.stream_distant();
            while let Ok(message) = rx.live.try_recv() {
                match message {
                    ServerMessage::World3d(_) => snapshots += 1,
                    ServerMessage::Chunk3d { .. } => chunks += 1,
                    ServerMessage::DistantView { .. } | ServerMessage::DistantTile { .. } => {
                        panic!("display traffic entered the live queue")
                    }
                    _ => {}
                }
            }
            assert!(world.players.contains_key(&1));
            assert_eq!(rx.far.len(), FAR_QUEUE_CAPACITY);
            assert!(world.distant_input().is_empty());
        }
        assert!(snapshots > 0 && chunks > 0);
        let mut bytes = 0;
        while let Ok(message) = rx.try_recv() {
            let ServerMessage::DistantTile { blocks, .. } = message else {
                panic!("unexpected view change")
            };
            bytes += blocks.len();
        }
        assert!(bytes <= SEND_BYTES);
        world.unregister_player(1);
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn sending_and_cache_payloads_have_hard_budgets() {
        let mut world = VoxelWorld::fixture(17);
        let mut rx = observer(&mut world, 1, [0, 0], 64);
        for x in 0..4 {
            world.distant.cache_source(Output {
                column: [x, 0],
                revision: 0,
                known: world.terrain.has_column([x, 0]),
                summary: Summary::from_column(vec![AIR; 32768], 1),
            });
        }
        world.stream_distant();
        let (mut bytes, mut count) = (0, 0);
        while let Ok(m) = rx.try_recv() {
            if let ServerMessage::DistantTile { blocks, .. } = m {
                bytes += blocks.len();
                count += 1;
            }
        }
        assert!(bytes <= SEND_BYTES && count <= BATCH_COLUMNS);
        assert!(count > 0);
        let summary = Summary::from_column(vec![AIR; 32768], 1);
        let mut state = State::default();
        for i in 0..1200 {
            state.cache_source(Output {
                column: [i, 0],
                revision: 0,
                known: false,
                summary: summary.clone(),
            });
            assert!(state.source_bytes <= SOURCE_BYTES);
        }
        assert!(!state.sources.contains_key(&[0, 0]));
        assert!(state.sources.len() <= SOURCE_ENTRIES);
        assert_eq!(
            state.source_bytes,
            state
                .sources
                .values()
                .map(|s| s.summary.bytes())
                .sum::<usize>()
        );
        for i in 0..TILE_ENTRIES + 1 {
            state.ensure_tile(Key {
                origin: [(i as i32 % 1024 - 512) * 32, (i as i32 / 1024) * 32],
                stride: 2,
            });
            assert!(state.tiles.len() <= TILE_ENTRIES && state.tile_bytes <= TILE_BYTES);
        }
        assert_eq!(
            state.tile_bytes,
            state
                .tiles
                .values()
                .map(|t| t.assembly.bytes())
                .sum::<usize>()
        );
    }
}
