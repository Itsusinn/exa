use exa_protocol::*;
use exa_simulation::{grid::Grid, step_player_in_grid};
use std::collections::{BTreeSet, VecDeque};

pub struct Client3d {
    pub terrain: Grid,
    pub distant: crate::distant::DistantCache,
    pub revision: u64,
    pub equipment_revision: u64,
    pub equipment_geometry_revision: u64,
    pub pipe_reply: Option<PipeRouteReply>,
    pub assembly_reply: Option<AssemblyReport>,
    pub distillation_reply: Option<DistillationReply>,
    equipment_server_revision: u64,
    authoritative_body: Body,
    pub dirty_chunks: BTreeSet<[i32; 3]>,
    pub player_id: u32,
    pub spawn_rotation: [f32; 2],
    pub body: Body,
    pub flying: bool,
    pub actors: Vec<Actor3d>,
    pub items: Vec<GroundItem3d>,
    pub hp: i32,
    pub inventory: PlayerInventory,
    pub log: Vec<LogEntry>,
    pub container: Option<ContainerSnapshot>,
    pub container_error: String,
    pub ready: bool,
    pub pending: VecDeque<(u32, PlayerInput)>,
    next_seq: u32,
    last_ack: u32,
}
impl Default for Client3d {
    fn default() -> Self {
        Self {
            terrain: Grid::unknown(),
            distant: crate::distant::DistantCache::default(),
            revision: 0,
            equipment_revision: 0,
            equipment_geometry_revision: 0,
            pipe_reply: None,
            assembly_reply: None,
            distillation_reply: None,
            equipment_server_revision: 0,
            authoritative_body: Body::default(),
            dirty_chunks: BTreeSet::new(),
            player_id: 0,
            spawn_rotation: [0.0, -0.1],
            body: Body::default(),
            flying: false,
            actors: vec![],
            items: vec![],
            hp: 100,
            inventory: PlayerInventory::default(),
            log: vec![],
            container: None,
            container_error: String::new(),
            ready: false,
            pending: VecDeque::new(),
            next_seq: 1,
            last_ack: 0,
        }
    }
}
impl Client3d {
    fn dirty_section(&mut self, origin: [i32; 3]) {
        self.dirty_chunks.insert(origin);
        // Corner AO reads the full one-cell halo, including diagonal sections.
        for x in [-CHUNK_EDGE, 0, CHUNK_EDGE] {
            for y in [-CHUNK_EDGE, 0, CHUNK_EDGE] {
                for z in [-CHUNK_EDGE, 0, CHUNK_EDGE] {
                    let adjacent = [origin[0] + x, origin[1] + y, origin[2] + z];
                    if exa_protocol::valid_voxel(adjacent) {
                        self.dirty_chunks.insert(adjacent);
                    }
                }
            }
        }
    }
    fn dirty_distant_ao(&mut self, column: [i32; 2]) {
        for y in (0..WORLD_HEIGHT).step_by(16) {
            for z in -1..=1 {
                for x in -1..=1 {
                    let p = [(column[0] + x) * 16, y, (column[1] + z) * 16];
                    if self.terrain.blocks.contains_key(&p) {
                        self.dirty_chunks.insert(p);
                    }
                }
            }
        }
    }

    pub fn apply(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::DistantView { epoch, center } => {
                let previous_fine: BTreeSet<_> = self
                    .distant
                    .desired
                    .iter()
                    .filter(|key| key.stride == 1)
                    .copied()
                    .collect();
                if self.distant.view(epoch, center) {
                    let current_fine: BTreeSet<_> = self
                        .distant
                        .desired
                        .iter()
                        .filter(|key| key.stride == 1)
                        .copied()
                        .collect();
                    // fine_block filters the desired set. Both removal and returning
                    // to a cached fine tile change its contribution to near AO.
                    for key in previous_fine.symmetric_difference(&current_fine) {
                        self.dirty_distant_ao([key.origin[0] / 16, key.origin[1] / 16]);
                    }
                }
            }
            ServerMessage::DistantTile {
                epoch,
                key,
                revision,
                blocks,
            } => {
                if self.distant.insert(epoch, key, revision, blocks) && key.stride == 1 {
                    // Never add remote render-only blocks to the gameplay Grid.
                    self.dirty_distant_ao([key.origin[0] / 16, key.origin[1] / 16]);
                }
            }
            ServerMessage::UnloadColumn { column } => {
                if !valid_column(column) {
                    return;
                }
                self.terrain.unload_column(column);
                let blocks = &self.terrain.blocks;
                let before = self.terrain.equipment.len();
                self.terrain.equipment.retain(|_, device| {
                    exa_simulation::equipment::occupied_cells(device)
                        .iter()
                        .any(|p| blocks.contains_key(&Grid::origin(*p)))
                });
                if before != self.terrain.equipment.len() {
                    self.equipment_revision += 1;
                    self.equipment_geometry_revision += 1;
                }
                self.revision += 1;
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    self.dirty_section([column[0] * 16, y, column[1] * 16]);
                }
            }
            ServerMessage::Welcome3d {
                player_id,
                rotation,
                ..
            } => {
                *self = Self::default();
                self.player_id = player_id;
                self.spawn_rotation = rotation;
            }
            ServerMessage::Chunk3d { origin, blocks } => {
                if self.terrain.apply_chunk(origin, &blocks) {
                    self.revision += 1;
                    self.dirty_section(origin);
                }
            }
            ServerMessage::EquipmentState { revision, devices } => {
                if revision < self.equipment_server_revision
                    || devices.len() > MAX_EQUIPMENT
                    || devices.iter().any(|device| !device.valid())
                {
                    return;
                }
                let next: std::collections::BTreeMap<_, _> = devices
                    .iter()
                    .cloned()
                    .map(|device| (device.id, device))
                    .collect();
                if next.len() != devices.len() {
                    return;
                }
                self.equipment_server_revision = revision;
                if self.terrain.equipment != next {
                    // Runtime inventories and readings do not affect collision geometry.
                    // Replaying movement at each process update would introduce needless
                    // corrections while a stationary pipe simply transports water.
                    let geometry_changed = self.terrain.equipment.len() != next.len()
                        || next.iter().any(|(id, device)| {
                            self.terrain.equipment.get(id).is_none_or(|old| {
                                old.kind != device.kind
                                    || old.origin != device.origin
                                    || old.axis != device.axis
                                    || old.pipe_ends != device.pipe_ends
                                    || old.tank_size != device.tank_size
                            })
                        });
                    self.terrain.equipment = next;
                    self.equipment_revision += 1;
                    if geometry_changed {
                        self.equipment_geometry_revision += 1;
                    }
                    if self.ready && geometry_changed {
                        self.replay_inputs();
                    }
                }
            }
            ServerMessage::PipeRouteReply(reply) => {
                if reply.valid() {
                    self.pipe_reply = Some(reply);
                }
            }
            ServerMessage::AssemblyReport(report) => {
                if report.valid() {
                    self.assembly_reply = Some(report);
                }
            }
            ServerMessage::Distillation(reply) => {
                if reply.valid() {
                    self.distillation_reply = Some(reply);
                }
            }
            ServerMessage::World3d(s) => {
                if s.seq < self.last_ack {
                    return;
                }
                self.last_ack = s.seq;
                while self.pending.front().is_some_and(|(seq, _)| *seq <= s.seq) {
                    self.pending.pop_front();
                }
                self.body = s.body;
                self.authoritative_body = s.body;
                self.flying = s.flying;
                self.actors = s.actors;
                self.items = s.items;
                self.hp = s.hp;
                self.inventory = s.inventory;
                self.log = s.log;
                self.container = s.container;
                self.container_error = s.container_error;
                self.ready = self.player_id != 0
                    && (self.ready || self.terrain.nearby_known(s.body.position));
                self.replay_inputs();
            }
            _ => {}
        }
    }
    fn replay_inputs(&mut self) {
        self.body = self.authoritative_body;
        let obstacles: Vec<_> = self
            .actors
            .iter()
            .filter(|a| a.id != self.player_id)
            .map(|a| a.position)
            .collect();
        for &(_, input) in &self.pending {
            if self.terrain.nearby_known(self.body.position) {
                step_player_in_grid(
                    &mut self.body,
                    input,
                    self.flying,
                    &self.terrain,
                    &obstacles,
                );
            }
        }
    }
    pub fn input(
        &mut self,
        input: PlayerInput,
        send: impl FnOnce(ClientMessage) -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.ready {
            return Err("Waiting for world data".into());
        }
        if self.pending.len() >= MAX_PENDING_INPUTS {
            return Err("Waiting for server input acknowledgement".into());
        }
        if !input.valid() || self.next_seq == u32::MAX {
            return Err("Input queue unavailable".into());
        }
        let seq = self.next_seq;
        send(ClientMessage::Input3d { seq, input })?;
        self.next_seq += 1;
        self.pending.push_back((seq, input));
        let obstacles: Vec<_> = self
            .actors
            .iter()
            .filter(|a| a.id != self.player_id)
            .map(|a| a.position)
            .collect();
        if self.terrain.nearby_known(self.body.position) {
            step_player_in_grid(
                &mut self.body,
                input,
                self.flying,
                &self.terrain,
                &obstacles,
            );
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn render_only_tiles_never_make_unknown_gameplay_space_traversable() {
        use exa_protocol::*;
        let mut model = super::Client3d::default();
        model.apply(ServerMessage::DistantView {
            epoch: 1,
            center: [-1, 0],
        });
        model.apply(ServerMessage::DistantTile {
            epoch: 1,
            key: DistantTileKey {
                origin: [-16, 0],
                stride: 1,
            },
            revision: 1,
            blocks: vec![AIR; 32768],
        });
        assert_eq!(model.distant.tiles.len(), 1);
        assert!(model.terrain.blocks.is_empty());
        assert!(model.terrain.solid([-1, 20, 1]));
        model.apply(ServerMessage::Welcome3d {
            player_id: 1,
            seed: 0,
            rotation: [0.0; 2],
        });
        assert!(model.distant.tiles.is_empty());
        assert_eq!(model.distant.epoch, 0);
    }

    #[test]
    fn fine_tile_membership_changes_invalidate_near_ao_even_when_raw_is_cached() {
        use exa_protocol::*;
        let mut model = super::Client3d::default();
        let tile = DistantTileKey {
            origin: [0, 0],
            stride: 1,
        };
        let near = [16, 0, 0];
        model.apply(ServerMessage::Chunk3d {
            origin: near,
            blocks: vec![AIR; 4096],
        });
        model.apply(ServerMessage::DistantView {
            epoch: 1,
            center: [0, 0],
        });
        model.apply(ServerMessage::DistantTile {
            epoch: 1,
            key: tile,
            revision: 1,
            blocks: vec![STONE; 32768],
        });
        assert_eq!(model.distant.fine_block([15, 1, 1]), STONE);
        model.dirty_chunks.clear();
        model.apply(ServerMessage::DistantView {
            epoch: 2,
            center: [10, 0],
        });
        assert!(model.distant.tiles.contains_key(&tile));
        assert_eq!(model.distant.fine_block([15, 1, 1]), 255);
        assert!(model.dirty_chunks.contains(&near));
        model.dirty_chunks.clear();
        model.apply(ServerMessage::DistantView {
            epoch: 3,
            center: [0, 0],
        });
        assert_eq!(model.distant.fine_block([15, 1, 1]), STONE);
        assert!(model.dirty_chunks.contains(&near));
        assert_eq!(model.terrain.get([16, 1, 1]), AIR);
        assert!(model.terrain.solid([15, 1, 1]));
    }
    use super::*;
    #[test]
    fn process_updates_keep_prediction_but_geometry_changes_replay() {
        let mut m = model();
        m.authoritative_body = m.body;
        let mut device = EquipmentSnapshot {
            id: 8,
            revision: 0,
            kind: EquipmentKind::Tank,
            origin: [15, 1, 15],
            axis: 0,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            thermal: None,
            pipe_ends: None,
            tank_size: None,
            column_design: None,
            reading: None,
        };
        m.apply(ServerMessage::EquipmentState {
            revision: 1,
            devices: vec![device.clone()],
        });
        m.body.position[0] += 0.25;
        let predicted = m.body;
        device.contents.push(MaterialAmount {
            material: "icheme:water".into(),
            moles: 100.0,
        });
        device.internal_energy_j = 1000.0;
        m.apply(ServerMessage::EquipmentState {
            revision: 2,
            devices: vec![device.clone()],
        });
        assert_eq!(m.body, predicted);
        assert_eq!(m.terrain.equipment[&8].contents, device.contents);
        device.origin[0] += 1;
        m.apply(ServerMessage::EquipmentState {
            revision: 3,
            devices: vec![device.clone()],
        });
        assert_eq!(m.body, m.authoritative_body);
        m.body.position[0] += 0.25;
        let geometry_revision = m.equipment_geometry_revision;
        device.tank_size = Some(TankSize {
            diameter: 4,
            height: 8,
        });
        m.apply(ServerMessage::EquipmentState {
            revision: 4,
            devices: vec![device],
        });
        assert_eq!(m.equipment_geometry_revision, geometry_revision + 1);
        assert_eq!(m.body, m.authoritative_body);
    }
    #[test]
    fn equipment_visibility_same_revision_unload_and_invalid_state() {
        let mut m = model();
        m.authoritative_body = m.body;
        let tank = EquipmentSnapshot {
            id: 7,
            revision: 0,
            kind: EquipmentKind::Tank,
            origin: [15, 1, 15],
            axis: 0,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            thermal: None,
            pipe_ends: None,
            tank_size: None,
            column_design: None,
            reading: None,
        };
        m.apply(ServerMessage::EquipmentState {
            revision: 2,
            devices: vec![tank.clone()],
        });
        assert_eq!(m.terrain.equipment.len(), 1);
        // A spanning tank remains while one of its columns is still visible.
        m.apply(ServerMessage::UnloadColumn { column: [0, 0] });
        assert_eq!(m.terrain.equipment.len(), 1);
        m.apply(ServerMessage::EquipmentState {
            revision: 2,
            devices: vec![],
        });
        assert!(m.terrain.equipment.is_empty());
        m.apply(ServerMessage::EquipmentState {
            revision: 1,
            devices: vec![tank.clone()],
        });
        assert!(m.terrain.equipment.is_empty());
        m.apply(ServerMessage::EquipmentState {
            revision: 3,
            devices: vec![tank.clone(), tank.clone()],
        });
        assert!(m.terrain.equipment.is_empty());
        m.apply(ServerMessage::EquipmentState {
            revision: 3,
            devices: vec![tank],
        });
        for column in [[1, 0], [0, 1], [1, 1]] {
            m.apply(ServerMessage::UnloadColumn { column });
        }
        assert!(m.terrain.equipment.is_empty());
        m.apply(ServerMessage::Welcome3d {
            player_id: 2,
            seed: 0,
            rotation: [0.0; 2],
        });
        assert_eq!(m.equipment_revision, 0);
        assert!(m.terrain.equipment.is_empty());
    }
    fn model() -> Client3d {
        let mut m = Client3d {
            ready: true,
            terrain: Grid::empty(),
            player_id: 1,
            ..Default::default()
        };
        for x in 0..64 {
            for z in 0..64 {
                m.terrain.set([x, 0, z], STONE);
            }
        }
        m.body = Body {
            position: [10.5, 1.0, 10.5],
            grounded: true,
            ..Default::default()
        };
        m
    }
    #[test]
    fn unacknowledged_inputs_stop_at_server_capacity_and_resume_after_execution() {
        let mut m = model();
        let input = PlayerInput {
            forward: 1.0,
            ..Default::default()
        };
        let mut first_body = m.body;
        for seq in 1..=16 {
            m.input(input, |message| {
                assert!(matches!(message, ClientMessage::Input3d { seq: sent, .. } if sent == seq));
                Ok(())
            })
            .unwrap();
            if seq == 1 {
                first_body = m.body;
            }
        }
        let predicted = m.body;
        for _ in 0..64 {
            assert!(m
                .input(input, |_| panic!("full window must not send"))
                .is_err());
            assert_eq!(m.body, predicted);
            assert_eq!(m.pending.len(), 16);
            assert_eq!(m.next_seq, 17);
        }
        let ack = Snapshot3d {
            seq: 1,
            body: first_body,
            flying: false,
            actors: vec![],
            items: vec![],
            hp: 100,
            inventory: PlayerInventory::default(),
            log: vec![],
            container: None,
            container_error: String::new(),
        };
        m.apply(ServerMessage::World3d(ack.clone()));
        assert_eq!(m.body, predicted);
        assert_eq!(m.pending.len(), 15);
        assert!(m.input(input, |_| Err("transport full".into())).is_err());
        assert_eq!(m.body, predicted);
        assert_eq!(m.next_seq, 17);
        m.input(input, |message| {
            assert!(matches!(message, ClientMessage::Input3d { seq: 17, .. }));
            Ok(())
        })
        .unwrap();
        // Repeated/stale snapshots cannot grant additional send capacity.
        m.apply(ServerMessage::World3d(ack.clone()));
        m.apply(ServerMessage::World3d(Snapshot3d { seq: 0, ..ack }));
        assert!(m
            .input(input, |_| panic!("duplicate ack released capacity"))
            .is_err());
        assert_eq!(m.pending.len(), 16);
    }

    #[test]
    fn authority_switches_flight_before_replay_and_unknown_teleport_waits_for_chunks() {
        let mut m = model();
        let input = PlayerInput {
            jump: true,
            sprint: true,
            ..Default::default()
        };
        m.input(input, |_| Ok(())).unwrap();
        let mut s = Snapshot3d {
            seq: 0,
            body: Body {
                position: [24.0, 15.0, 24.0],
                ..Default::default()
            },
            flying: true,
            actors: vec![],
            items: vec![],
            hp: 100,
            inventory: PlayerInventory::default(),
            log: vec![],
            container: None,
            container_error: String::new(),
        };
        let mut expected = s.body;
        step_player_in_grid(&mut expected, input, true, &m.terrain, &[]);
        m.apply(ServerMessage::World3d(s.clone()));
        assert!(m.flying);
        assert_eq!(m.body, expected);
        assert_eq!(m.pending.len(), 1);
        s.body.position = [-500.0, 20.0, 500.0];
        m.apply(ServerMessage::World3d(s.clone()));
        assert_eq!(
            m.body, s.body,
            "unknown destination must not resolve a body inside unknown solid cells"
        );
        m.input(input, |_| Ok(())).unwrap();
        assert_eq!(m.body, s.body);
        let c = column_at(s.body.position);
        for z in -1..=1 {
            for x in -1..=1 {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    m.apply(ServerMessage::Chunk3d {
                        origin: [(c[0] + x) * 16, y, (c[1] + z) * 16],
                        blocks: vec![AIR; 4096],
                    });
                }
            }
        }
        s.seq = 1;
        m.apply(ServerMessage::World3d(s.clone()));
        assert!(m.body.position[1] > s.body.position[1]);
        assert_eq!(m.pending.len(), 1);
        s.seq = 2;
        s.flying = false;
        m.apply(ServerMessage::World3d(s));
        assert!(!m.flying);
        assert!(m.pending.is_empty());
    }

    #[test]
    fn failed_queue_does_not_predict_and_ack_replays_remaining_input() {
        let mut m = model();
        let old = m.body;
        let input = PlayerInput {
            forward: 1.0,
            ..Default::default()
        };
        assert!(m.input(input, |_| Err("full".into())).is_err());
        assert_eq!(m.body, old);
        assert!(m.pending.is_empty());
        m.input(input, |_| Ok(())).unwrap();
        let authoritative = m.body;
        m.input(input, |_| Ok(())).unwrap();
        let predicted = m.body;
        m.apply(ServerMessage::World3d(Snapshot3d {
            flying: false,
            seq: 1,
            body: authoritative,
            actors: vec![],
            items: vec![],
            hp: 100,
            inventory: PlayerInventory::default(),
            log: vec![],
            container: None,
            container_error: String::new(),
        }));
        assert_eq!(m.body, predicted);
        assert_eq!(m.pending.len(), 1);
    }
    #[test]
    fn unknown_space_blocks_prediction_and_bad_chunks_do_not_unlock_it() {
        let mut m = Client3d::default();
        let revision = m.revision;
        m.apply(ServerMessage::Chunk3d {
            origin: [-15, 0, 0],
            blocks: vec![AIR; 4096],
        });
        assert_eq!(m.revision, revision);
        assert!(m.terrain.solid([10, 10, 10]));
        assert!(!m.ready);
    }

    #[test]
    fn unload_removes_collision_data_and_meshes_and_new_session_discards_cache() {
        let mut m = model();
        m.apply(ServerMessage::Chunk3d {
            origin: [-16, 0, 0],
            blocks: vec![AIR; 4096],
        });
        assert!(!m.terrain.solid([-1, 1, 1]));
        m.dirty_chunks.clear();
        m.apply(ServerMessage::UnloadColumn { column: [-1, 0] });
        assert!(m.terrain.solid([-1, 1, 1]));
        assert!(m.dirty_chunks.contains(&[-16, 0, 0]));
        assert!(m.dirty_chunks.contains(&[0, 0, 0]));
        m.apply(ServerMessage::Welcome3d {
            player_id: 7,
            seed: 1,
            rotation: [0.0, 0.0],
        });
        assert!(m.terrain.blocks.is_empty());
        assert!(!m.ready);
    }

    #[test]
    fn changed_chunk_updates_collision_and_invalidates_only_the_ao_halo() {
        let mut m = model();
        m.terrain.set([15, 2, 8], STONE);
        let mut updated = m.terrain.clone();
        updated.set([15, 2, 8], AIR);
        m.apply(ServerMessage::Chunk3d {
            origin: [0, 0, 0],
            blocks: updated.chunk([0, 0, 0]),
        });
        assert!(!m.terrain.solid([15, 2, 8]));
        assert_eq!(m.dirty_chunks.len(), 18);
        assert!(m.dirty_chunks.contains(&[-16, 16, -16]));
        assert!(m.dirty_chunks.contains(&[16, 0, 16]));
        assert!(!m.dirty_chunks.contains(&[48, 0, 48]));
    }
}
