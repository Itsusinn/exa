//! Two-click pipe construction. Validate the complete proposal before changing any fact.
use super::*;
use game_simulation::{
    equipment::{collision_boxes, connected, ports},
    pipe_route, HEIGHT, RADIUS,
};

impl VoxelWorld {
    pub(super) fn pipe_command(&mut self, player: u32, command: EquipmentCommand) {
        self.expire_pipe_starts();
        let request_id = match &command {
            EquipmentCommand::BeginPipe { request_id, .. }
            | EquipmentCommand::LayPipe { request_id, .. }
            | EquipmentCommand::CancelPipe { request_id } => *request_id,
            _ => return,
        };
        if request_id == 0 || request_id > i64::MAX as u64 {
            if let Some(p) = self.players.get_mut(&player) {
                p.container_error = "无效铺管请求编号。".into();
            }
            return;
        }
        let Some(p) = self.players.get(&player) else {
            return;
        };
        if let Some(last) = &p.pipe_reply {
            if request_id == last.request_id {
                let _ = p.tx.try_send(ServerMessage::PipeRouteReply(last.clone()));
                return;
            }
            if request_id < last.request_id {
                let _ = p.tx.try_send(ServerMessage::PipeRouteReply(PipeRouteReply {
                    request_id,
                    stage: PipeRouteStage::Rejected,
                    anchor: p.pipe_start,
                    message: "铺管请求已过期。".into(),
                }));
                return;
            }
        }
        let result = self.apply_pipe_command(player, command);
        let (stage, mut message) = match result {
            Ok(stage) => (stage, String::new()),
            Err(e) => (PipeRouteStage::Rejected, e.to_string()),
        };
        while message.len() > 256 {
            message.pop();
        }
        let Some(p) = self.players.get_mut(&player) else {
            return;
        };
        let reply = PipeRouteReply {
            request_id,
            stage,
            anchor: p.pipe_start,
            message,
        };
        p.container_error = reply.message.clone();
        p.pipe_reply = Some(reply.clone());
        let _ = p.tx.try_send(ServerMessage::PipeRouteReply(reply));
    }

    fn apply_pipe_command(
        &mut self,
        player: u32,
        command: EquipmentCommand,
    ) -> Result<PipeRouteStage> {
        let p = self.players.get(&player).context("玩家尚未连接。")?;
        if matches!(command, EquipmentCommand::CancelPipe { .. }) {
            self.players.get_mut(&player).unwrap().pipe_start = None;
            return Ok(PipeRouteStage::Cancelled);
        }
        anyhow::ensure!(p.hp > 0 && p.open_container.is_none(), "当前无法铺管。");
        anyhow::ensure!(
            usize::from(p.inventory.selected) < HOTBAR_SLOTS
                && p.inventory
                    .slots
                    .get(usize::from(p.inventory.selected))
                    .and_then(Option::as_ref)
                    .is_some_and(|item| item.name == PIPE_ITEM && item.quantity > 0),
            "请先选择快捷栏中的管道。"
        );
        match command {
            EquipmentCommand::BeginPipe { yaw, pitch, .. } => {
                let anchor = pipe_route::resolve_anchor(&self.terrain, p.body.position, yaw, pitch)
                    .map_err(anyhow::Error::msg)?;
                self.require_open_pipe_anchor(anchor)?;
                self.players.get_mut(&player).unwrap().pipe_start = Some(anchor);
                Ok(PipeRouteStage::Started)
            }
            EquipmentCommand::LayPipe {
                start,
                end,
                cells,
                yaw,
                pitch,
                inventory_revision,
                ..
            } => {
                anyhow::ensure!(p.pipe_start == Some(start), "铺管起点与服务端记录不符。");
                self.require_open_pipe_anchor(start)?;
                self.require_open_pipe_anchor(end)?;
                let start_port = self.pipe_anchor_port(start)?;
                anyhow::ensure!(
                    distance(p.body.position, start_port.position) <= PIPE_START_RANGE,
                    "离铺管起点过远。"
                );
                let aimed = pipe_route::resolve_anchor(&self.terrain, p.body.position, yaw, pitch)
                    .map_err(anyhow::Error::msg)?;
                anyhow::ensure!(aimed == end, "终点已变化、被遮挡或超距。");
                anyhow::ensure!(self.tick >= p.edit_after, "操作过快，请稍后重试。");
                anyhow::ensure!(
                    inventory_revision == p.inventory.revision,
                    "物品栏已变化，请重试。"
                );
                let ends = pipe_route::validate(&self.terrain, start, end, &cells)
                    .map_err(anyhow::Error::msg)?;
                anyhow::ensure!(
                    !cells.is_empty()
                        && cells.len() <= MAX_PIPE_ROUTE
                        && self.terrain.equipment.len() + cells.len() <= MAX_EQUIPMENT,
                    "管路长度或设备数量超出上限。"
                );
                let revision = inventory::next_revision(self.industry_revision)?;
                let mut next_id = self.next_equipment_id;
                let mut devices = Vec::with_capacity(cells.len());
                for (&origin, pipe_ends) in cells.iter().zip(ends) {
                    let equipment = EquipmentSnapshot {
                        id: next_id,
                        revision,
                        kind: EquipmentKind::Pipe,
                        origin,
                        axis: pipe_ends[0] / 2,
                        pipe_ends: Some(pipe_ends),
                        tank_size: None,
                        contents: vec![],
                        internal_energy_j: 0.0,
                        enabled: false,
                        opening: 1.0,
                        pump_reversed: false,
                        thermal: None,
                        reading: None,
                    };
                    anyhow::ensure!(equipment.valid(), "管段位置或接头无效。");
                    anyhow::ensure!(
                        self.terrain.get(origin) == AIR && !self.equipment_conflicts(&equipment),
                        "管路与设备、地形或未知区域冲突。"
                    );
                    anyhow::ensure!(
                        !self
                            .items
                            .iter()
                            .any(|item| item.position.map(|v| v.floor() as i32) == origin),
                        "请先拾取管路位置的物品。"
                    );
                    let boxes = collision_boxes(&equipment);
                    let collides = |feet: [f32; 3]| {
                        boxes.iter().any(|b| {
                            let low = [feet[0] - RADIUS, feet[1], feet[2] - RADIUS];
                            let high = [feet[0] + RADIUS, feet[1] + HEIGHT, feet[2] + RADIUS];
                            (0..3).all(|i| low[i] < b.max[i] && high[i] > b.min[i])
                        })
                    };
                    anyhow::ensure!(
                        !self.players.values().any(|p| collides(p.body.position))
                            && !self
                                .npcs
                                .query::<&Npc>()
                                .iter()
                                .any(|n| collides(n.body.position)),
                        "不能在角色身体内铺管。"
                    );
                    next_id = next_id
                        .checked_add(1)
                        .filter(|id| *id <= i64::MAX as u64)
                        .context("设备标识已耗尽。")?;
                    devices.push(equipment);
                }
                let mut bag = p.inventory.clone();
                let mut needed = cells.len() as u32;
                for slot in &mut bag.slots {
                    if let Some(item) = slot {
                        if item.name == PIPE_ITEM {
                            let taken = needed.min(item.quantity);
                            item.quantity -= taken;
                            needed -= taken;
                            if item.quantity == 0 {
                                *slot = None;
                            }
                        }
                    }
                }
                anyhow::ensure!(needed == 0, "背包中的管道数量不足，未放置任何管段。");
                bag.revision = inventory::next_revision(bag.revision)?;
                // No fallible operation follows: inventory, IDs and every pipe commit together.
                for device in devices {
                    self.terrain.equipment.insert(device.id, device);
                }
                self.industry_revision = revision;
                self.next_equipment_id = next_id;
                let p = self.players.get_mut(&player).unwrap();
                p.inventory = bag;
                p.edit_after = self.tick + 6;
                p.pipe_start = None;
                Ok(PipeRouteStage::Laid)
            }
            _ => unreachable!(),
        }
    }

    fn pipe_anchor_port(
        &self,
        anchor: PipeAnchor,
    ) -> Result<game_simulation::equipment::EquipmentPort> {
        let d = self
            .terrain
            .equipment
            .get(&anchor.id)
            .context("铺管端点设备已移除。")?;
        anyhow::ensure!(d.revision == anchor.revision, "铺管端点设备已变化。");
        ports(d)
            .get(usize::from(anchor.port))
            .copied()
            .context("铺管端口不存在。")
    }
    fn require_open_pipe_anchor(&self, anchor: PipeAnchor) -> Result<()> {
        let port = self.pipe_anchor_port(anchor)?;
        anyhow::ensure!(
            !self
                .terrain
                .equipment
                .values()
                .filter(|d| d.id != anchor.id)
                .flat_map(ports)
                .any(|other| connected(port, other)),
            "铺管端口已经连接。"
        );
        Ok(())
    }
    pub(super) fn expire_pipe_starts(&mut self) {
        let expired: Vec<_> = self
            .players
            .iter()
            .filter_map(|(&id, p)| {
                p.pipe_start
                    .filter(|anchor| {
                        p.hp <= 0
                            || self.pipe_anchor_port(*anchor).is_err()
                            || self.pipe_anchor_port(*anchor).is_ok_and(|port| {
                                distance(p.body.position, port.position) > PIPE_START_RANGE
                            })
                            || self.require_open_pipe_anchor(*anchor).is_err()
                    })
                    .map(|_| id)
            })
            .collect();
        for id in expired {
            self.cancel_pipe_start(id, "铺管起点已失效或距离过远。");
        }
    }
    pub(super) fn cancel_pipe_start(&mut self, player: u32, message: &str) {
        let Some(p) = self.players.get_mut(&player) else {
            return;
        };
        if p.pipe_start.take().is_none() {
            return;
        }
        if let Some(last) = &p.pipe_reply {
            let reply = PipeRouteReply {
                request_id: last.request_id,
                stage: PipeRouteStage::Cancelled,
                anchor: None,
                message: message.into(),
            };
            let _ = p.tx.try_send(ServerMessage::PipeRouteReply(reply.clone()));
            p.pipe_reply = Some(reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(id: u64, kind: EquipmentKind, origin: [i32; 3]) -> EquipmentSnapshot {
        EquipmentSnapshot {
            id,
            revision: 0,
            kind,
            origin,
            axis: 0,
            pipe_ends: None,
            tank_size: None,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            thermal: None,
            reading: None,
        }
    }
    fn world() -> (VoxelWorld, mpsc::Receiver<ServerMessage>) {
        let mut grid = Grid::unknown();
        for x in [-16, 0, 16] {
            for z in [-16, 0, 16] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    grid.apply_chunk([x, y, z], &vec![AIR; 4096]);
                }
                for dx in 0..16 {
                    for dz in 0..16 {
                        grid.set([x + dx, 80, z + dz], STONE);
                    }
                }
            }
        }
        grid.equipment
            .insert(1, device(1, EquipmentKind::Pump, [-2, 81, 0]));
        grid.equipment
            .insert(2, device(2, EquipmentKind::Pump, [2, 82, 0]));
        let mut w = VoxelWorld::with_map(19, grid);
        w.npcs.clear();
        w.items.clear();
        w.next_equipment_id = 3;
        let (tx, rx) = mpsc::channel(256);
        w.register_profile(1, [11; 16], "builder".into(), tx)
            .unwrap();
        w.industry_kit(1).unwrap();
        let pipe_slot = w.players[&1]
            .inventory
            .slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|item| item.name == PIPE_ITEM))
            .unwrap();
        w.players.get_mut(&1).unwrap().inventory.selected = pipe_slot as u8;
        w.players.get_mut(&1).unwrap().body.position = [-0.5, 81.0, 2.5];
        (w, rx)
    }
    fn aim(w: &VoxelWorld, point: [f32; 3]) -> (f32, f32) {
        let p = w.players[&1].body.position;
        let d = [
            point[0] - p[0],
            point[1] - p[1] - game_simulation::EYE_HEIGHT,
            point[2] - p[2],
        ];
        (
            d[0].atan2(-d[2]).rem_euclid(std::f32::consts::TAU),
            d[1].atan2(d[0].hypot(d[2])),
        )
    }
    fn begin(w: &mut VoxelWorld) -> PipeAnchor {
        let (yaw, pitch) = aim(w, [-1.1, 81.5, 0.5]);
        w.pipe_command(
            1,
            EquipmentCommand::BeginPipe {
                request_id: 1,
                yaw,
                pitch,
            },
        );
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Started
        );
        w.players[&1].pipe_start.unwrap()
    }
    fn lay(w: &mut VoxelWorld, start: PipeAnchor, request_id: u64) -> EquipmentCommand {
        w.players.get_mut(&1).unwrap().body.position = [0.5, 82.0, 2.5];
        let (yaw, pitch) = aim(w, [2.1, 82.5, 0.5]);
        EquipmentCommand::LayPipe {
            request_id,
            start,
            end: PipeAnchor {
                id: 2,
                revision: 0,
                port: 0,
            },
            cells: vec![[-1, 81, 0], [0, 81, 0], [0, 82, 0], [1, 82, 0]],
            yaw,
            pitch,
            inventory_revision: w.players[&1].inventory.revision,
        }
    }
    fn pipe_count(w: &VoxelWorld) -> u32 {
        w.players[&1]
            .inventory
            .slots
            .iter()
            .flatten()
            .filter(|i| i.name == PIPE_ITEM)
            .map(|i| i.quantity)
            .sum()
    }
    #[test]
    fn routed_build_is_atomic_and_retries_never_charge_twice() {
        let (mut w, _) = world();
        w.pipe_command(
            1,
            EquipmentCommand::CancelPipe {
                request_id: u64::MAX,
            },
        );
        assert!(w.players[&1].pipe_reply.is_none());
        let start = begin(&mut w);
        let before = pipe_count(&w);
        let command = lay(&mut w, start, 2);
        let bag_revision = w.players[&1].inventory.revision;
        w.pipe_command(1, command.clone());
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Laid
        );
        assert_eq!(pipe_count(&w), before - 4);
        assert_eq!(w.players[&1].inventory.revision, bag_revision + 1);
        assert_eq!(w.terrain.equipment.len(), 6);
        assert_eq!(w.next_equipment_id, 7);
        assert_eq!(w.terrain.equipment[&4].pipe_ends, Some([0, 3]));
        assert_eq!(w.terrain.equipment[&5].pipe_ends, Some([1, 2]));
        let facts = w.terrain.equipment.clone();
        // Idempotent replies precede held-item admission: changing slots after
        // a successful operation must not make its retry perform a new build.
        w.players.get_mut(&1).unwrap().inventory.selected = 0;
        let bag = w.players[&1].inventory.clone();
        w.pipe_command(1, command);
        assert_eq!(w.terrain.equipment, facts);
        assert_eq!(w.players[&1].inventory, bag);
        w.pipe_command(1, EquipmentCommand::CancelPipe { request_id: 1 });
        assert_eq!(w.terrain.equipment, facts);
        assert_eq!(w.players[&1].inventory, bag);
    }
    #[test]
    fn both_route_clicks_require_held_pipe_but_cancel_always_remains_available() {
        let (mut w, _rx) = world();
        let pipe_slot = w.players[&1].inventory.selected;
        w.players.get_mut(&1).unwrap().inventory.selected = 0;
        let (yaw, pitch) = aim(&w, [-1.1, 81.5, 0.5]);
        let facts = w.terrain.equipment.clone();
        let initial_bag = w.players[&1].inventory.clone();
        w.pipe_command(
            1,
            EquipmentCommand::BeginPipe {
                request_id: 1,
                yaw,
                pitch,
            },
        );
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Rejected
        );
        assert!(w.players[&1].pipe_start.is_none());
        assert_eq!(w.players[&1].inventory, initial_bag);
        assert_eq!(w.terrain.equipment, facts);
        assert_eq!(w.next_equipment_id, 3);
        w.players.get_mut(&1).unwrap().inventory.selected = pipe_slot;
        w.pipe_command(
            1,
            EquipmentCommand::BeginPipe {
                request_id: 2,
                yaw,
                pitch,
            },
        );
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Started
        );
        let start = w.players[&1].pipe_start.unwrap();
        w.players.get_mut(&1).unwrap().inventory.selected = 0;
        let command = lay(&mut w, start, 3);
        let bag = w.players[&1].inventory.clone();
        w.pipe_command(1, command);
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Rejected
        );
        assert_eq!(w.players[&1].pipe_start, Some(start));
        assert_eq!(w.players[&1].inventory, bag);
        assert_eq!(w.terrain.equipment, facts);
        assert_eq!(w.next_equipment_id, 3);
        w.pipe_command(1, EquipmentCommand::CancelPipe { request_id: 4 });
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Cancelled
        );
        assert!(w.players[&1].pipe_start.is_none());
        assert_eq!(w.players[&1].inventory, bag);
    }
    #[test]
    fn rejected_route_preserves_start_inventory_ids_and_cached_rejection() {
        let (mut w, _) = world();
        let start = begin(&mut w);
        let command = lay(&mut w, start, 2);
        w.terrain.set([0, 82, 0], STONE);
        let facts = w.terrain.equipment.clone();
        let bag = w.players[&1].inventory.clone();
        w.pipe_command(1, command.clone());
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Rejected
        );
        assert_eq!(w.players[&1].pipe_start, Some(start));
        assert_eq!(w.terrain.equipment, facts);
        assert_eq!(w.players[&1].inventory, bag);
        assert_eq!(w.next_equipment_id, 3);
        w.terrain.set([0, 82, 0], AIR);
        w.pipe_command(1, command);
        assert_eq!(w.terrain.equipment, facts);
        assert_eq!(w.players[&1].inventory, bag);
        let retry = lay(&mut w, start, 3);
        w.pipe_command(1, retry);
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Laid
        );
    }
    #[test]
    fn stale_inventory_death_and_distance_cancel_preserve_construction_facts() {
        let (mut w, _rx) = world();
        let start = begin(&mut w);
        let command = lay(&mut w, start, 2);
        w.players.get_mut(&1).unwrap().inventory.revision += 1;
        let bag = w.players[&1].inventory.clone();
        w.pipe_command(1, command);
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Rejected
        );
        assert_eq!(w.players[&1].inventory, bag);
        assert_eq!(w.terrain.equipment.len(), 2);
        w.players.get_mut(&1).unwrap().hp = 0;
        w.tick();
        assert!(w.players[&1].pipe_start.is_none());
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Cancelled
        );
        w.players.get_mut(&1).unwrap().pipe_start = Some(start);
        w.players.get_mut(&1).unwrap().body.position = [40.0, 81.0, 0.5];
        w.expire_pipe_starts();
        assert!(w.players[&1].pipe_start.is_none());
    }
    #[test]
    fn rack_and_horizontal_pipe_share_one_cell_and_roundtrip_without_refunds() {
        let (mut w, _) = world();
        w.terrain
            .equipment
            .insert(3, device(3, EquipmentKind::PipeRack, [-1, 81, 0]));
        w.next_equipment_id = 4;
        let start = begin(&mut w);
        let command = lay(&mut w, start, 2);
        w.pipe_command(1, command);
        assert_eq!(
            w.players[&1].pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Laid
        );
        assert!(game_simulation::equipment::rack_compatible(
            &w.terrain.equipment[&3],
            &w.terrain.equipment[&4]
        ));
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("world3d.bin");
        w.save(&path).unwrap();
        let restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.terrain.equipment, w.terrain.equipment);
    }
}
