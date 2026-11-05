mod assembly;
mod commands;
mod containers;
mod editing;
mod industry;
mod inventory;
mod persistence;
mod pipe_laying;
pub use persistence::lock_world;
#[cfg(test)]
mod legacy_fixture;
#[cfg(test)]
mod spawn_tests;
mod streaming;
mod terrain;
use anyhow::{Context, Result};
use exa_protocol::*;
#[cfg(test)]
use exa_simulation::overlaps;
use exa_simulation::{clear_line, clear_line_grid, distance, grid::Grid};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::Path,
};
use tokio::sync::mpsc;

struct Player {
    sent_columns: BTreeSet<[i32; 2]>,
    industry_columns: BTreeSet<[i32; 2]>,
    industry_revision: Option<u64>,
    pipe_start: Option<PipeAnchor>,
    pipe_reply: Option<PipeRouteReply>,
    assembly_report: Option<AssemblyReport>,
    assembly_after: u64,
    uuid: Option<[u8; 16]>,
    name: String,
    body: Body,
    flying: bool,
    command_after: u64,
    command_log: Vec<LogEntry>,
    hp: i32,
    yaw: f32,
    pitch: f32,
    last_seq: u32,
    received: u32,
    inbox: VecDeque<(u32, PlayerInput)>,
    tx: mpsc::Sender<ServerMessage>,
    inventory: PlayerInventory,
    attack_tick: u64,
    open_container: Option<[i32; 3]>,
    edit_after: u64,
    container_error: String,
}
struct Npc {
    uuid: [u8; 16],
    actor: Actor3d,
    body: Body,
    attack_tick: u64,
}

pub struct VoxelWorld {
    pub world_seed: u64,
    pub terrain: Grid,
    players: HashMap<u32, Player>,
    npcs: hecs::World,
    items: Vec<GroundItem3d>,
    log: Vec<LogEntry>,
    next_id: u32,
    tick: u64,
    dirty_chunks: BTreeSet<[i32; 3]>,
    containers: BTreeMap<[i32; 3], ContainerSnapshot>,
    saved_players: BTreeMap<[u8; 16], crate::player_data::PlayerData>,
    save_path: Option<std::path::PathBuf>,
    spawn_sites: Vec<[f32; 3]>,
    industry_revision: u64,
    next_equipment_id: u64,
    industry_ledger: crate::industry_data::ExchangeLedger,
    assembly_structure: Vec<crate::assembly::Structure>,
}
impl VoxelWorld {
    /// Validate the embedded generation assets before accepting a session.
    pub fn validate_generation() -> Result<()> {
        terrain::validate_structures()
    }
    pub fn new(seed: u64) -> Result<Self> {
        let mut world = Self::with_map(seed, terrain::generate(seed)?);
        world.npcs.clear();
        let spawn = *world.spawn_sites.first().context("no safe spawn surface")?;
        world.items[0].position = spawn;
        // Retain the initial empty chest, but place it on nearby natural ground.
        let chest = world
            .spawn_sites
            .iter()
            .copied()
            .find(|p| {
                let d = distance(*p, spawn);
                (2.0..=3.0).contains(&d)
                    && (p[1] - spawn[1]).abs() <= 1.0
                    && clear_line(
                        [spawn[0], spawn[1] + 1.58, spawn[2]],
                        [p[0], p[1] + 0.5, p[2]],
                        |p| world.terrain.solid(p),
                    )
            })
            .or_else(|| {
                world
                    .spawn_sites
                    .iter()
                    .copied()
                    .filter(|p| distance(*p, spawn) >= 2.0)
                    .min_by(|a, b| distance(*a, spawn).total_cmp(&distance(*b, spawn)))
            });
        if let Some(position) = chest {
            world.terrain.set(position.map(|v| v.floor() as i32), CHEST);
            world.containers = containers::empty_containers(&world.terrain);
        }
        let mut positions = Vec::new();
        for p in world.spawn_sites.iter().copied() {
            if distance(p, spawn) < 18.0 || positions.iter().any(|q| distance(*q, p) < 4.0) {
                continue;
            }
            positions.push(p);
            if positions.len() == 3 {
                break;
            }
        }
        for (i, p) in positions.into_iter().enumerate() {
            world.npcs.spawn((Npc {
                uuid: *uuid::Uuid::new_v4().as_bytes(),
                actor: Actor3d {
                    id: (1 << 31) + i as u32,
                    kind: 1,
                    position: p,
                    yaw: 0.0,
                    hp: 30,
                    max_hp: 30,
                },
                body: Body {
                    position: p,
                    grounded: true,
                    ..Default::default()
                },
                attack_tick: 0,
            },));
        }
        Ok(world)
    }
    #[cfg(test)]
    fn fixture(seed: u64) -> Self {
        Self::with_map(seed, legacy_fixture::generate(seed))
    }
    fn with_map(seed: u64, terrain: Grid) -> Self {
        let spawn_sites = terrain::spawn_sites(&terrain);
        let containers = containers::empty_containers(&terrain);
        // Legacy saves without entity archives retain their original actors.
        // New worlds replace these positions with safe natural surfaces above.
        let mut npcs = hecs::World::new();
        for (i, p) in [[31.5, 7.0, 21.5], [41.5, 7.0, 27.5], [19.5, 7.0, 42.5]]
            .into_iter()
            .enumerate()
        {
            npcs.spawn((Npc {
                uuid: *uuid::Uuid::new_v4().as_bytes(),
                actor: Actor3d {
                    id: (1 << 31) + i as u32,
                    kind: 1,
                    position: p,
                    yaw: 0.0,
                    hp: 30,
                    max_hp: 30,
                },
                body: Body {
                    position: p,
                    ..Default::default()
                },
                attack_tick: 0,
            },));
        }
        Self {
            world_seed: seed,
            terrain,
            players: HashMap::new(),
            npcs,
            items: vec![GroundItem3d {
                id: 1,
                position: [30.5, 7.0, 30.5],
                name: "Field supplies".into(),
            }],
            log: vec![],
            next_id: 1,
            tick: 0,
            dirty_chunks: BTreeSet::new(),
            containers,
            saved_players: BTreeMap::new(),
            save_path: None,
            spawn_sites,
            industry_revision: 0,
            industry_ledger: crate::industry_data::ExchangeLedger::default(),
            next_equipment_id: 1,
            assembly_structure: vec![],
        }
    }
    pub fn player_count(&self) -> usize {
        self.players.len()
    }
    pub fn allocate_player_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .filter(|id| *id < 1 << 31)
            .expect("player ID exhausted");
        id
    }
    pub fn register_player(&mut self, id: u32, tx: mpsc::Sender<ServerMessage>) -> Result<()> {
        anyhow::ensure!(self.players.len() < 16, "world is full");
        let position = self
            .spawn_position(id)
            .context("no unoccupied spawn position")?;
        self.players.insert(
            id,
            Player {
                sent_columns: BTreeSet::new(),
                industry_columns: BTreeSet::new(),
                industry_revision: None,
                pipe_start: None,
                pipe_reply: None,
                assembly_report: None,
                assembly_after: 0,
                uuid: None,
                name: String::new(),
                body: Body {
                    position,
                    grounded: true,
                    ..Default::default()
                },
                hp: 100,
                flying: false,
                command_after: 0,
                command_log: vec![],
                yaw: 0.0,
                pitch: -0.1,
                last_seq: 0,
                received: 0,
                inbox: VecDeque::new(),
                tx,
                inventory: PlayerInventory::default(),
                attack_tick: 0,
                open_container: None,
                edit_after: 0,
                container_error: String::new(),
            },
        );
        Ok(())
    }
    pub fn unregister_player(&mut self, id: u32) {
        if let Some(p) = self.players.remove(&id) {
            if let Some(uuid) = p.uuid {
                match p.saved_data(uuid) {
                    Ok(data) => {
                        self.saved_players.insert(uuid, data);
                    }
                    Err(e) => tracing::error!(error=%e, "cannot capture disconnected player"),
                }
            }
        }
    }
    fn spawn_position(&self, exclude: u32) -> Option<[f32; 3]> {
        self.spawn_sites
            .iter()
            .filter_map(|p| {
                terrain::standing_position(&self.terrain, p[0].floor() as i32, p[2].floor() as i32)
            })
            .find(|&position| {
                self.players
                    .iter()
                    .all(|(&id, p)| id == exclude || distance(p.body.position, position) > 1.0)
                    && self
                        .npcs
                        .query::<&Npc>()
                        .iter()
                        .all(|n| distance(n.body.position, position) > 1.0)
            })
    }
    pub fn input(&mut self, id: u32, seq: u32, input: PlayerInput) -> Result<()> {
        let p = self.players.get_mut(&id).context("unknown player")?;
        anyhow::ensure!(input.valid(), "invalid movement input");
        anyhow::ensure!(
            seq == p
                .received
                .checked_add(1)
                .context("input sequence exhausted")?,
            "out-of-order input"
        );
        anyhow::ensure!(
            p.inbox.len() < MAX_PENDING_INPUTS,
            "client sends inputs faster than simulation"
        );
        p.received = seq;
        p.inbox.push_back((seq, input));
        Ok(())
    }
    pub fn chat(&mut self, id: u32, text: String) -> Result<()> {
        anyhow::ensure!(text.len() <= 512, "chat too long");
        if self.players.contains_key(&id) {
            let text = text.trim();
            if text.starts_with('/') {
                self.player_command(id, text);
            } else if !text.is_empty() {
                self.note(format!("Player {id}: {text}"));
            }
        }
        Ok(())
    }
    fn note(&mut self, text: String) {
        self.log.push(LogEntry {
            text,
            color: 0xBEC9B2,
            turn: self.tick,
        });
        if self.log.len() > 8 {
            self.log.remove(0);
        }
    }
    pub fn tick(&mut self) {
        self.tick += 1;
        self.stream_chunks();
        self.broadcast_terrain();
        self.broadcast_equipment();
        let mut actions = Vec::new();
        let mut occupants: Vec<_> = self
            .players
            .iter()
            .map(|(&id, p)| (id, p.body.position))
            .collect();
        occupants.extend(
            self.npcs
                .query::<&Npc>()
                .iter()
                .map(|n| (n.actor.id, n.body.position)),
        );
        let mut ids: Vec<_> = self.players.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let obstacles: Vec<_> = occupants
                .iter()
                .filter(|a| a.0 != id)
                .map(|a| a.1)
                .collect();
            let p = self.players.get_mut(&id).unwrap();
            let input = if let Some((seq, input)) = p.inbox.pop_front() {
                p.last_seq = seq;
                input
            } else {
                PlayerInput {
                    yaw: p.yaw,
                    pitch: p.pitch,
                    ..PlayerInput::default()
                }
            };
            p.yaw = input.yaw;
            p.pitch = input.pitch;
            exa_simulation::step_player_in_grid(
                &mut p.body,
                input,
                p.flying,
                &self.terrain,
                &obstacles,
            );
            if let Some(a) = occupants.iter_mut().find(|a| a.0 == id) {
                a.1 = p.body.position;
            }
            if input.attack || input.pickup {
                actions.push((id, input));
            }
        }
        for (id, input) in actions {
            self.action(id, input);
        }
        for npc in self.npcs.query_mut::<&mut Npc>() {
            let target = self
                .players
                .iter()
                .filter(|(_, p)| p.hp > 0)
                .min_by(|a, b| {
                    distance(a.1.body.position, npc.body.position)
                        .total_cmp(&distance(b.1.body.position, npc.body.position))
                });
            let mut input = PlayerInput::default();
            if let Some((&id, p)) = target {
                let d = distance(p.body.position, npc.body.position);
                let a = [
                    npc.body.position[0],
                    npc.body.position[1] + 1.0,
                    npc.body.position[2],
                ];
                let b = [
                    p.body.position[0],
                    p.body.position[1] + 1.0,
                    p.body.position[2],
                ];
                if d < 13.0 && clear_line_grid(a, b, &self.terrain) {
                    input.yaw = (p.body.position[0] - npc.body.position[0])
                        .atan2(npc.body.position[2] - p.body.position[2]);
                    if d > 1.25 {
                        input.forward = 0.32;
                    }
                    if d < 1.65 && self.tick >= npc.attack_tick + 30 {
                        npc.attack_tick = self.tick;
                        let p = self.players.get_mut(&id).unwrap();
                        p.hp = (p.hp - 4).max(0);
                    }
                }
            }
            let obstacles: Vec<_> = occupants
                .iter()
                .filter(|a| a.0 != npc.actor.id)
                .map(|a| a.1)
                .collect();
            exa_simulation::step_in_grid(&mut npc.body, input, &self.terrain, &obstacles);
            if let Some(a) = occupants.iter_mut().find(|a| a.0 == npc.actor.id) {
                a.1 = npc.body.position;
            }
            npc.actor.position = npc.body.position;
            npc.actor.yaw = input.yaw;
        }
        let dead: Vec<_> = self
            .players
            .iter()
            .filter(|(_, p)| p.hp == 0 || p.body.position[1] < -2.0)
            .map(|(&id, _)| id)
            .collect();
        for id in dead {
            self.cancel_pipe_start(id, "死亡或复活已取消铺管起点。");
            self.players.get_mut(&id).unwrap().open_container = None;
            if let Some(position) = self.spawn_position(id) {
                let p = self.players.get_mut(&id).unwrap();
                p.body = Body {
                    position,
                    grounded: true,
                    ..Default::default()
                };
                p.hp = 100;
                p.flying = false;
            }
        }
        for p in self.players.values_mut() {
            if p.open_container.is_some_and(|position| {
                !exa_simulation::container_reachable(&self.terrain, p.body.position, position)
            }) {
                p.open_container = None;
                p.container_error = "距离箱子过远或被遮挡，已关闭。".into();
            }
        }
        let mut actors: Vec<_> = self
            .players
            .iter()
            .map(|(&id, p)| Actor3d {
                id,
                kind: 0,
                position: p.body.position,
                yaw: p.yaw,
                hp: p.hp,
                max_hp: 100,
            })
            .collect();
        actors.extend(self.npcs.query::<&Npc>().iter().map(|n| n.actor.clone()));
        actors.sort_by_key(|a| a.id);
        let mut slow = Vec::new();
        for (&id, p) in &self.players {
            if p.tx
                .try_send(ServerMessage::World3d(Snapshot3d {
                    seq: p.last_seq,
                    body: p.body,
                    flying: p.flying,
                    actors: actors.clone(),
                    items: self.items.clone(),
                    hp: p.hp,
                    inventory: p.inventory.clone(),
                    log: self.player_log(p),
                    container: p
                        .open_container
                        .and_then(|pos| self.containers.get(&pos).cloned()),
                    container_error: p.container_error.clone(),
                }))
                .is_err()
            {
                slow.push(id);
            }
        }
        for id in slow {
            self.unregister_player(id);
        }
    }
    fn action(&mut self, id: u32, input: PlayerInput) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let pos = p.body.position;
        if input.pickup {
            if let Some(i) = self.items.iter().position(|item| {
                distance(item.position, pos) < 2.0
                    && clear_line_grid(
                        [pos[0], pos[1] + 0.5, pos[2]],
                        [item.position[0], item.position[1] + 0.2, item.position[2]],
                        &self.terrain,
                    )
            }) {
                self.pickup_item(id, i);
            }
        }
        if !input.attack || self.tick < self.players[&id].attack_tick + 12 {
            return;
        }
        self.players.get_mut(&id).unwrap().attack_tick = self.tick;
        let eye = [pos[0], pos[1] + exa_simulation::EYE_HEIGHT, pos[2]];
        let direction = [
            input.yaw.sin() * input.pitch.cos(),
            input.pitch.sin(),
            -input.yaw.cos() * input.pitch.cos(),
        ];
        let target = self
            .npcs
            .query::<&Npc>()
            .iter()
            .filter_map(|n| {
                let center = [
                    n.body.position[0],
                    n.body.position[1] + 1.0,
                    n.body.position[2],
                ];
                let d = distance(eye, center);
                let dot = (0..3)
                    .map(|i| (center[i] - eye[i]) * direction[i])
                    .sum::<f32>()
                    / d.max(0.001);
                (d < 2.8 && dot > 0.65 && clear_line_grid(eye, center, &self.terrain))
                    .then_some((n.actor.id, d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|a| a.0);
        if let Some(target) = target {
            let mut killed = None;
            for (handle, n) in self.npcs.query_mut::<(hecs::Entity, &mut Npc)>() {
                if n.actor.id == target {
                    n.actor.hp -= 10;
                    if n.actor.hp <= 0 {
                        killed = Some(handle);
                    }
                }
            }
            if let Some(handle) = killed {
                let _ = self.npcs.despawn(handle);
            }
            self.note(format!(
                "Player {id} hit a zombie for 10{}",
                if killed.is_some() {
                    " (defeated)."
                } else {
                    "."
                }
            ));
        }
    }
    pub fn load(path: &Path) -> Result<Self> {
        Self::validate_generation()?;
        let bytes = std::fs::read(path).context("read 3D world")?;
        anyhow::ensure!(bytes.starts_with(b"DNG3D001"),"not a 3D save; legacy 2D saves are preserved and require explicit migration. Use a new EXA_SAVE_PATH");
        let saved: Save = rmp_serde::from_slice(&bytes[8..]).context("decode 3D world")?;
        anyhow::ensure!(
            matches!(saved.version, 1..=13)
                && saved.size
                    == if saved.version >= 7 {
                        [0, WORLD_HEIGHT, 0]
                    } else if saved.version == 6 {
                        [0, 32, 0]
                    } else {
                        WORLD_SIZE
                    },
            "unsupported 3D save version/dimensions"
        );
        anyhow::ensure!(
            saved.version >= 5 || saved.entity_archive.is_none(),
            "unexpected entity reference in legacy world"
        );
        anyhow::ensure!(
            saved.version >= 8 || saved.industry_archive.is_none(),
            "unexpected industry reference in legacy world"
        );
        if saved.version >= 4 {
            anyhow::ensure!(
                saved.blocks.is_empty() && saved.containers.is_empty() && saved.items.is_none(),
                "flat data in chunk-based world"
            );
            let generation = saved.chunk_archive.context("missing chunk checkpoint")?;
            anyhow::ensure!(
                saved.player_archive.as_ref() == Some(&generation),
                "chunk/player checkpoint mismatch"
            );
            anyhow::ensure!(
                uuid::Uuid::parse_str(&generation)?.to_string() == generation,
                "invalid chunk checkpoint reference"
            );
            let archive = crate::chunk_data::read(
                &path
                    .with_extension("chunks")
                    .join(format!("{generation}.rkyv")),
            )?;
            anyhow::ensure!(
                archive.generation == generation && archive.seed == saved.seed,
                "chunk checkpoint identity mismatch"
            );
            anyhow::ensure!(
                archive.data_version
                    == if saved.version >= 7 {
                        3
                    } else if saved.version == 6 {
                        2
                    } else {
                        1
                    },
                "world/chunk version mismatch"
            );
            let decoded = archive.restore()?;
            if saved.version >= 6 {
                let columns = saved
                    .generated_columns
                    .context("missing generated column manifest")?;
                anyhow::ensure!(
                    columns.len() == decoded.terrain.columns().len()
                        && columns.into_iter().collect::<BTreeSet<_>>()
                            == decoded.terrain.columns(),
                    "missing or unexpected generated columns"
                );
            }
            let mut world = Self::with_map(saved.seed, decoded.terrain);
            world.restore_containers(decoded.containers)?;
            world.load_player_checkpoint(path, generation.clone(), decoded.items)?;
            world.tick = archive.last_update;
            if saved.version >= 5 {
                anyhow::ensure!(
                    saved.entity_archive.as_ref() == Some(&generation),
                    "entity checkpoint mismatch"
                );
                world.load_entity_checkpoint(path, &generation)?;
            }
            if saved.version >= 8 {
                anyhow::ensure!(
                    saved.industry_archive.as_ref() == Some(&generation),
                    "industry checkpoint mismatch"
                );
                world.load_industry_checkpoint(path, &generation, saved.version)?;
            }
            if saved.version < 13 {
                containers::backup_version(path, &bytes, saved.version)?;
            }
            return Ok(world);
        }
        anyhow::ensure!(
            saved.chunk_archive.is_none(),
            "unexpected chunk reference in legacy world"
        );
        anyhow::ensure!(
            saved.blocks.len() == (64 * 32 * 64)
                && saved
                    .blocks
                    .iter()
                    .all(|b| *b <= if saved.version == 1 { PLANK } else { CHEST }),
            "invalid voxel data"
        );
        let mut world = Self::with_map(saved.seed, Grid::from_legacy(&saved.blocks));
        if saved.version == 1 {
            anyhow::ensure!(
                saved.containers.is_empty(),
                "v1 save contains unexpected container data"
            );
            world.add_migration_chest()?;
        } else {
            world.restore_containers(saved.containers)?;
        }
        if saved.version == 3 {
            world.load_player_checkpoint(
                path,
                saved.player_archive.context("missing player checkpoint")?,
                saved.items.context("missing saved ground items")?,
            )?;
        } else {
            anyhow::ensure!(
                saved.player_archive.is_none() && saved.items.is_none(),
                "unexpected player data in legacy world"
            );
        }
        containers::backup_version(path, &bytes, saved.version)?;
        Ok(world)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let player_archive = self.write_player_checkpoint(path)?;
        let chunks = crate::chunk_data::ChunkArchive::capture(
            &self.terrain,
            &self.containers,
            &self.items,
            self.world_seed,
            player_archive.clone(),
            self.tick,
        )?;
        let folder = path.with_extension("chunks");
        std::fs::create_dir_all(&folder)?;
        crate::chunk_data::write_new(&folder.join(format!("{player_archive}.rkyv")), &chunks)?;
        self.write_entity_checkpoint(path, &player_archive)?;
        self.write_industry_checkpoint(path, &player_archive)?;
        let saved = Save {
            version: 13,
            size: [0, WORLD_HEIGHT, 0],
            generated_columns: Some(self.terrain.columns().into_iter().collect()),
            seed: self.world_seed,
            blocks: vec![],
            containers: vec![],
            chunk_archive: Some(player_archive.clone()),
            entity_archive: Some(player_archive.clone()),
            industry_archive: Some(player_archive.clone()),
            player_archive: Some(player_archive),
            items: None,
        };
        let mut data = b"DNG3D001".to_vec();
        data.extend(rmp_serde::to_vec(&saved)?);
        let temp = path.with_extension("tmp");
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        f.write_all(&data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(temp, path).context("replace 3D save")?;
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
struct Save {
    version: u32,
    size: [i32; 3],
    seed: u64,
    blocks: Vec<u8>,
    #[serde(default)]
    containers: Vec<ContainerSnapshot>,
    #[serde(default)]
    player_archive: Option<String>,
    #[serde(default)]
    items: Option<Vec<GroundItem3d>>,
    #[serde(default)]
    chunk_archive: Option<String>,
    #[serde(default)]
    entity_archive: Option<String>,
    #[serde(default)]
    generated_columns: Option<Vec<[i32; 2]>>,
    #[serde(default)]
    industry_archive: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enemy_damage_respawns_player_without_losing_inventory() {
        let mut w = VoxelWorld::fixture(1);
        let (tx, mut rx) = mpsc::channel(64);
        w.register_player(1, tx).unwrap();
        while rx.try_recv().is_ok() {}
        w.npcs.clear();
        let position = [28.5, 7.0, 31.2];
        w.npcs.spawn((Npc {
            uuid: *uuid::Uuid::new_v4().as_bytes(),
            actor: Actor3d {
                id: 1 << 31,
                kind: 1,
                position,
                yaw: 0.0,
                hp: 30,
                max_hp: 30,
            },
            body: Body {
                position,
                grounded: true,
                ..Default::default()
            },
            attack_tick: 0,
        },));
        w.players.get_mut(&1).unwrap().hp = 8;
        w.players.get_mut(&1).unwrap().inventory.slots[0] = Some(InventoryEntry {
            name: "Supplies".into(),
            quantity: 1,
        });
        w.tick = 29;
        w.tick();
        assert_eq!(
            w.players[&1].hp, 4,
            "enemy attack applies authoritative damage"
        );
        w.tick = 59;
        w.tick();
        assert_eq!(w.players[&1].hp, 100, "lethal damage respawns player");
        assert_eq!(
            w.players[&1].inventory.slots[0].as_ref().unwrap().quantity,
            1
        );
        assert!(!overlaps(w.players[&1].body.position, &|p| w
            .terrain
            .solid(p)));
    }
    #[test]
    fn melee_requires_reach_facing_line_of_sight_and_cooldown() {
        let mut w = VoxelWorld::fixture(1);
        let (tx, _rx) = mpsc::channel(64);
        w.register_player(1, tx).unwrap();
        w.npcs.clear();
        let position = [28.5, 7.0, 30.9];
        let npc = w.npcs.spawn((Npc {
            uuid: *uuid::Uuid::new_v4().as_bytes(),
            actor: Actor3d {
                id: 1 << 31,
                kind: 1,
                position,
                yaw: 0.0,
                hp: 30,
                max_hp: 30,
            },
            body: Body {
                position,
                ..Default::default()
            },
            attack_tick: 0,
        },));
        let attack = PlayerInput {
            attack: true,
            ..Default::default()
        };
        w.tick = 100;
        w.terrain.set([28, 8, 31], STONE);
        w.action(1, attack);
        assert_eq!(w.npcs.get::<&Npc>(npc).unwrap().actor.hp, 30);
        w.terrain.set([28, 8, 31], AIR);
        w.tick += 12;
        w.action(
            1,
            PlayerInput {
                yaw: std::f32::consts::PI,
                ..attack
            },
        );
        assert_eq!(w.npcs.get::<&Npc>(npc).unwrap().actor.hp, 30);
        w.tick += 12;
        w.action(1, attack);
        assert_eq!(w.npcs.get::<&Npc>(npc).unwrap().actor.hp, 20);
        w.action(1, attack);
        assert_eq!(w.npcs.get::<&Npc>(npc).unwrap().actor.hp, 20);
        w.tick += 12;
        w.action(1, attack);
        w.tick += 12;
        w.action(1, attack);
        assert!(!w.npcs.contains(npc));
    }
    #[test]
    fn pickup_is_authoritative_and_reconnect_uses_a_free_spawn() {
        let mut w = VoxelWorld::fixture(1);
        let (tx1, _rx1) = mpsc::channel(64);
        let (tx2, _rx2) = mpsc::channel(64);
        let (tx3, _rx3) = mpsc::channel(64);
        w.register_player(1, tx1).unwrap();
        w.register_player(2, tx2).unwrap();
        w.unregister_player(1);
        w.register_player(3, tx3).unwrap();
        assert!(distance(w.players[&2].body.position, w.players[&3].body.position) > 1.0);
        w.items[0].position = w.players[&3].body.position;
        for _ in 0..2 {
            w.action(
                3,
                PlayerInput {
                    pickup: true,
                    ..Default::default()
                },
            );
        }
        assert!(w.items.is_empty());
        assert_eq!(w.players[&3].inventory.len(), 1);
    }
    #[test]
    fn slow_clients_are_removed_without_blocking_the_tick() {
        let mut w = VoxelWorld::fixture(1);
        let (tx, _rx) = mpsc::channel(32);
        for seq in 0..32 {
            tx.try_send(ServerMessage::Pong { seq }).unwrap();
        }
        w.register_player(1, tx).unwrap();
        w.tick();
        assert_eq!(w.player_count(), 0);
    }
    #[test]
    fn save_roundtrip_rejects_legacy_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("world.bin");
        let w = VoxelWorld::fixture(42);
        w.save(&path).unwrap();
        let loaded = VoxelWorld::load(&path).unwrap();
        assert_eq!(loaded.terrain.blocks, w.terrain.blocks);
        loaded.save(&path).unwrap();
        std::fs::write(&path, b"legacy").unwrap();
        assert!(VoxelWorld::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"legacy");
    }
    #[test]
    fn input_overflow_is_rejected_without_advancing_sequence_or_simulation() {
        let mut w = VoxelWorld::fixture(42);
        let (tx, _rx) = mpsc::channel(64);
        w.register_player(1, tx).unwrap();
        let start = w.players[&1].body;
        for seq in 1..=MAX_PENDING_INPUTS as u32 {
            w.input(1, seq, PlayerInput::default()).unwrap();
        }
        let next = MAX_PENDING_INPUTS as u32 + 1;
        let error = w.input(1, next, PlayerInput::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "client sends inputs faster than simulation"
        );
        assert_eq!(w.players[&1].body, start);
        assert_eq!(w.players[&1].last_seq, 0);
        assert_eq!(w.players[&1].received, next - 1);
        w.tick();
        assert_eq!(w.players[&1].last_seq, 1);
        w.input(1, next, PlayerInput::default()).unwrap();
        assert_eq!(w.players[&1].inbox.len(), MAX_PENDING_INPUTS);
    }

    #[test]
    fn server_consumes_at_most_one_input_per_tick_and_acks_execution() {
        let mut w = VoxelWorld::fixture(42);
        let (tx, mut rx) = mpsc::channel(64);
        w.register_player(1, tx).unwrap();
        let start = w.players[&1].body.position;
        for seq in 1..=4 {
            w.input(
                1,
                seq,
                PlayerInput {
                    forward: 1.0,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(w.players[&1].last_seq, 0);
        w.tick();
        assert_eq!(w.players[&1].last_seq, 1);
        assert!(distance(start, w.players[&1].body.position) < 0.15);
        while rx.try_recv().is_ok() {}
        assert!(w
            .input(
                1,
                5,
                PlayerInput {
                    forward: 2.0,
                    ..Default::default()
                }
            )
            .is_err());
        assert!(w.input(1, 4, PlayerInput::default()).is_err());
    }
}
