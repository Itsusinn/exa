use crate::local_server::{LocalEvent, LocalServer};
use crate::network_client::{NetworkEvent, QuicSession, Transport};
use game_client_core::voxel::Client3d;
pub type Model = Client3d;
use game_simulation::{equipment, grid::Grid, step_in_grid};
use protocol::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct PipePlanKey {
    start: PipeAnchor,
    end: PipeAnchor,
    terrain: u64,
    geometry: u64,
}

pub struct Session {
    pub model: Client3d,
    pub transport: Option<Box<dyn Transport>>,
    pub address: String,
    pub name: String,
    pub identity_path: std::path::PathBuf,
    pub certificate: String,
    pub status: String,
    pub demo: bool,
    pub generation: u64,
    pub equipment_axis: u8,
    pub tank_size: TankSize,
    pub assembly_report: Option<AssemblyReport>,
    pub assembly_pending: bool,
    assembly_request: u64,
    assembly_master: Option<u64>,
    pub distillation_open: bool,
    pub distillation_reply: Option<DistillationReply>,
    pub distillation_spec: Option<DistillationSpec>,
    pub distillation_pending: bool,
    pub distillation_message: String,
    distillation_request: u64,
    distillation_active: Option<u64>,
    distillation_cancel_pending: Option<u64>,
    pub pipe_mode: bool,
    pub pipe_start: Option<PipeAnchor>,
    pub pipe_target: Option<PipeAnchor>,
    pub pipe_pending: Option<u64>,
    pub pipe_status: String,
    pub pipe_plan: Vec<([i32; 3], [u8; 2])>,
    pub pipe_plan_revision: u64,
    pipe_request: u64,
    pipe_plan_key: Option<PipePlanKey>,
    local_server: Option<LocalServer>,
    pub local_error: bool,
    retired_transports: Vec<Box<dyn Transport>>,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            model: Client3d::default(),
            transport: None,
            address: "127.0.0.1:9876".into(),
            name: "Explorer".into(),
            identity_path: crate::identity::default_path(),
            certificate: "save/server.cert.der".into(),
            status: "Connect to explore the 3D world.".into(),
            demo: false,
            generation: 0,
            equipment_axis: 0,
            tank_size: TankSize::default(),
            assembly_report: None,
            assembly_pending: false,
            assembly_request: 0,
            assembly_master: None,
            distillation_open: false,
            distillation_reply: None,
            distillation_spec: None,
            distillation_pending: false,
            distillation_message: String::new(),
            distillation_request: 0,
            distillation_active: None,
            distillation_cancel_pending: None,
            pipe_mode: false,
            pipe_start: None,
            pipe_target: None,
            pipe_pending: None,
            pipe_status: String::new(),
            pipe_plan: vec![],
            pipe_plan_revision: 0,
            pipe_request: 0,
            pipe_plan_key: None,
            local_server: None,
            local_error: false,
            retired_transports: Vec::new(),
        }
    }
}
impl Session {
    pub fn shutdown_pending(&self) -> bool {
        self.local_server.as_ref().is_some_and(|s| s.stopping)
            || !self.retired_transports.is_empty()
    }

    fn retire_transport(&mut self) {
        if let Some(mut transport) = self.transport.take() {
            transport.close();
            self.retired_transports.push(transport);
        }
    }

    pub fn local_running(&self) -> bool {
        self.local_server.is_some()
    }

    pub fn start_singleplayer(&mut self, path: std::path::PathBuf) {
        self.disconnect();
        if self.local_running() {
            return;
        }
        self.local_error = false;
        match LocalServer::start(path) {
            Ok(server) => {
                self.local_server = Some(server);
                self.status = "正在启动单人世界…".into();
            }
            Err(e) => {
                self.local_error = true;
                self.status = format!("单人游戏启动失败：{e:#}");
            }
        }
    }

    pub fn connect(&mut self) {
        self.disconnect();
        if self.local_running() {
            return;
        }
        self.local_error = false;
        let result = (|| -> anyhow::Result<QuicSession> {
            QuicSession::connect_file_identity(
                self.address.parse()?,
                self.name.clone(),
                self.certificate.clone(),
                self.identity_path.clone(),
            )
        })();
        match result {
            Ok(t) => {
                self.transport = Some(Box::new(t));
                self.status = "Connecting...".into();
            }
            Err(e) => self.status = format!("Connection failed: {e:#}"),
        }
    }
    pub fn disconnect(&mut self) {
        if let Some(server) = &mut self.local_server {
            server.stop();
        }
        self.retire_transport();
        self.model = Client3d::default();
        self.demo = false;
        self.equipment_axis = 0;
        self.tank_size = TankSize::default();
        self.clear_assembly();
        self.assembly_request = 0;
        self.clear_distillation();
        self.distillation_request = 0;
        self.distillation_cancel_pending = None;
        self.clear_pipe_route();
        self.pipe_request = 0;
        self.generation += 1;
        self.status = if self.local_running() {
            "正在保存并关闭单人世界…"
        } else {
            "Disconnected"
        }
        .into();
    }
    pub fn tick(&mut self) {
        self.retired_transports
            .retain(|transport| !transport.finished());
        if let Some(server) = &mut self.local_server {
            match server.poll() {
                Some(LocalEvent::Ready(address, certificate)) if !server.stopping => {
                    match QuicSession::connect_identity(
                        address,
                        self.name.clone(),
                        certificate,
                        self.identity_path.clone(),
                    ) {
                        Ok(transport) => {
                            self.transport = Some(Box::new(transport));
                        }
                        Err(e) => {
                            self.status = format!("本地连接失败：{e:#}");
                            self.local_error = true;
                            server.stop();
                        }
                    }
                }
                Some(LocalEvent::Finished(result)) => {
                    self.local_server = None;
                    if let Err(e) = result {
                        self.local_error = true;
                        self.status = format!("单人世界错误：{e:#}");
                    } else if !self.local_error {
                        self.status = "单人世界已保存并关闭".into();
                    }
                }
                _ => {}
            }
        }
        let mut disconnected = false;
        if let Some(t) = &mut self.transport {
            for _ in 0..256 {
                match t.poll() {
                    Some(NetworkEvent::Message(m)) => {
                        self.model.apply(m);
                        if self.model.ready {
                            self.status = "Connected / 3D world".into();
                        }
                    }
                    Some(NetworkEvent::Disconnected(reason)) => {
                        self.status = reason;
                        disconnected = true;
                        break;
                    }
                    None => break,
                }
            }
        }
        if disconnected {
            self.retire_transport();
            self.model.ready = false;
            self.model.terrain.equipment.clear();
            self.model.equipment_revision += 1;
            self.generation += 1;
            self.clear_pipe_route();
            self.clear_assembly();
            self.clear_distillation();
            self.distillation_cancel_pending = None;
            if let Some(server) = &mut self.local_server {
                self.local_error = true;
                server.stop();
            }
        }
        if let Some(reply) = self.model.pipe_reply.take() {
            self.accept_pipe_reply(reply);
        }
        if let Some(reply) = self.model.assembly_reply.take() {
            self.accept_assembly_reply(reply);
        }
        if let Some(reply) = self.model.distillation_reply.take() {
            self.accept_distillation_reply(reply);
        }
        self.flush_distillation_cancel();
    }
    pub fn open_distillation(&mut self) -> bool {
        if !self.model.ready || self.demo || self.model.hp <= 0 {
            return false;
        }
        self.distillation_open = true;
        true
    }
    fn clear_distillation(&mut self) {
        self.distillation_open = false;
        self.distillation_reply = None;
        self.distillation_spec = None;
        self.distillation_pending = false;
        self.distillation_message.clear();
        self.distillation_active = None;
        self.model.distillation_reply = None;
    }
    pub fn close_distillation(&mut self) {
        self.cancel_distillation();
        self.clear_distillation();
    }
    fn flush_distillation_cancel(&mut self) {
        if let (Some(request_id), Some(transport)) =
            (self.distillation_cancel_pending, &self.transport)
        {
            if transport
                .send(ClientMessage::Distillation(DistillationCommand::Cancel {
                    request_id,
                }))
                .is_ok()
            {
                self.distillation_cancel_pending = None;
            }
        }
    }
    pub fn cancel_distillation(&mut self) {
        if self.distillation_pending {
            self.distillation_cancel_pending = self.distillation_active;
        }
        self.distillation_active = None;
        self.distillation_pending = false;
        self.distillation_reply = None;
        self.distillation_message = "已取消请求。".into();
        self.flush_distillation_cancel();
    }
    pub fn calculate_distillation(&mut self, spec: DistillationSpec) -> bool {
        if !self.distillation_open || !self.model.ready || self.demo || self.model.hp <= 0 {
            return false;
        }
        self.flush_distillation_cancel();
        if self.distillation_cancel_pending.is_some() {
            self.distillation_message = "等待取消请求进入发送队列，请稍后重试。".into();
            return false;
        }
        if !spec.valid() {
            self.distillation_message = "参数超出计算器支持范围，请检查输入。".into();
            return false;
        }
        if self.distillation_pending {
            self.distillation_message = "请等待当前计算，或先取消。".into();
            return false;
        }
        let Some(request_id) = self
            .distillation_request
            .checked_add(1)
            .filter(|id| *id <= i64::MAX as u64)
        else {
            self.distillation_message = "计算请求编号已用尽，请重连。".into();
            return false;
        };
        let Some(transport) = &self.transport else {
            return false;
        };
        match transport.send(ClientMessage::Distillation(
            DistillationCommand::Calculate {
                request_id,
                spec: spec.clone(),
            },
        )) {
            Ok(()) => {
                self.distillation_request = request_id;
                self.distillation_active = Some(request_id);
                self.distillation_spec = Some(spec);
                self.distillation_reply = None;
                self.distillation_pending = true;
                self.distillation_message.clear();
                true
            }
            Err(error) => {
                self.distillation_message = error;
                false
            }
        }
    }
    fn accept_distillation_reply(&mut self, reply: DistillationReply) {
        if !self.distillation_open
            || self.distillation_active != Some(reply.request_id)
            || !reply.valid()
        {
            return;
        }
        if self
            .distillation_reply
            .as_ref()
            .is_some_and(|old| old.status != DistillationStatus::Running)
        {
            return;
        }
        self.distillation_pending = reply.status == DistillationStatus::Running;
        self.distillation_message.clear();
        self.distillation_reply = Some(reply);
    }
    pub fn clear_assembly(&mut self) {
        self.assembly_master = None;
        self.assembly_report = None;
        self.assembly_pending = false;
        self.model.assembly_reply = None;
    }

    fn accept_assembly_reply(&mut self, reply: AssemblyReport) {
        if self.assembly_master == Some(reply.master_id)
            && reply.request_id == self.assembly_request
            && reply.valid()
        {
            self.assembly_pending = false;
            self.assembly_report = Some(reply);
        }
    }

    pub fn check_assembly(&mut self, id: u64, revision: u64, yaw: f32, pitch: f32) {
        if !self.model.ready || self.demo {
            return;
        }
        let Some(request_id) = self
            .assembly_request
            .checked_add(1)
            .filter(|id| *id <= i64::MAX as u64)
        else {
            self.model.container_error = "结构检查请求编号已用尽，请重连。".into();
            return;
        };
        let Some(transport) = &self.transport else {
            return;
        };
        match transport.send(ClientMessage::Equipment(EquipmentCommand::CheckAssembly {
            request_id,
            id,
            revision,
            yaw,
            pitch,
        })) {
            Ok(()) => {
                self.assembly_request = request_id;
                self.assembly_master = Some(id);
                self.assembly_report = None;
                self.assembly_pending = true;
            }
            Err(error) => self.model.container_error = error,
        }
    }
    fn clear_pipe_route(&mut self) {
        self.pipe_mode = false;
        self.pipe_start = None;
        self.pipe_target = None;
        self.pipe_pending = None;
        self.pipe_plan.clear();
        self.pipe_plan_key = None;
        self.pipe_status.clear();
        self.pipe_plan_revision = self.pipe_plan_revision.wrapping_add(1);
    }

    fn send_pipe(&mut self, make: impl FnOnce(u64) -> EquipmentCommand) -> bool {
        let Some(id) = self
            .pipe_request
            .checked_add(1)
            .filter(|id| *id <= i64::MAX as u64)
        else {
            self.pipe_status = "铺管请求编号已用尽，请重连。".into();
            return false;
        };
        self.pipe_request = id;
        let Some(transport) = &self.transport else {
            return false;
        };
        match transport.send(ClientMessage::Equipment(make(id))) {
            Ok(()) => {
                self.pipe_pending = Some(id);
                true
            }
            Err(error) => {
                self.pipe_status = error;
                false
            }
        }
    }

    pub fn toggle_pipe_mode(&mut self) {
        if self.pipe_mode {
            self.cancel_pipe_route();
            return;
        }
        if !self.model.ready || self.demo || self.selected_equipment() != Some(EquipmentKind::Pipe)
        {
            self.pipe_status = "请先选择快捷栏中的管道。".into();
            return;
        }
        self.clear_pipe_route();
        self.pipe_mode = true;
        self.pipe_status = "铺管：右键选择空闲接口；Esc / P 取消。".into();
    }

    pub fn cancel_pipe_route(&mut self) {
        let active = self.pipe_mode || self.pipe_start.is_some() || self.pipe_pending.is_some();
        self.clear_pipe_route();
        if active && self.model.ready {
            // A newer cancellation supersedes a queued begin/lay response. Never
            // retry a timed-out lay automatically: the authority may have committed it.
            self.send_pipe(|request_id| EquipmentCommand::CancelPipe { request_id });
        }
    }

    fn accept_pipe_reply(&mut self, reply: PipeRouteReply) {
        if reply.stage == PipeRouteStage::Cancelled
            && reply.request_id == self.pipe_request
            && (self.pipe_pending == Some(reply.request_id) || self.pipe_start.is_some())
        {
            self.clear_pipe_route();
            self.pipe_status = reply.message;
            return;
        }
        if self.pipe_pending != Some(reply.request_id) {
            return;
        }
        self.pipe_pending = None;
        self.pipe_status = reply.message;
        match reply.stage {
            PipeRouteStage::Started if self.pipe_mode => {
                self.pipe_start = reply.anchor;
                self.pipe_plan_key = None;
            }
            PipeRouteStage::Laid => {
                self.pipe_start = None;
                self.pipe_target = None;
                self.pipe_plan.clear();
                self.pipe_plan_key = None;
                self.pipe_plan_revision = self.pipe_plan_revision.wrapping_add(1);
            }
            PipeRouteStage::Rejected => {
                self.pipe_start = reply.anchor;
                self.pipe_target = None;
                self.pipe_plan.clear();
                self.pipe_plan_key = None;
                self.pipe_plan_revision = self.pipe_plan_revision.wrapping_add(1);
            }
            PipeRouteStage::Cancelled => {
                self.pipe_start = None;
            }
            _ => {}
        }
    }

    pub fn update_pipe_route(&mut self, yaw: f32, pitch: f32) {
        if !self.pipe_mode {
            return;
        }
        if !self.model.ready
            || self.model.hp <= 0
            || self.selected_equipment() != Some(EquipmentKind::Pipe)
        {
            self.cancel_pipe_route();
            return;
        }
        if let Some(start) = self.pipe_start {
            let too_far = self
                .model
                .terrain
                .equipment
                .get(&start.id)
                .is_none_or(|device| {
                    equipment::ports(device)
                        .get(usize::from(start.port))
                        .is_none_or(|port| {
                            game_simulation::distance(port.position, self.model.body.position)
                                > PIPE_START_RANGE
                        })
                });
            if too_far {
                self.cancel_pipe_route();
                return;
            }
        }
        if self.pipe_pending.is_some() {
            return;
        }
        let target = game_simulation::pipe_route::resolve_anchor(
            &self.model.terrain,
            self.model.body.position,
            yaw,
            pitch,
        );
        self.pipe_target = target.as_ref().ok().copied();
        let Some(start) = self.pipe_start else {
            return;
        };
        let end = match target {
            Ok(anchor) => anchor,
            Err(error) => {
                if !self.pipe_plan.is_empty() {
                    self.pipe_plan_revision = self.pipe_plan_revision.wrapping_add(1);
                }
                self.pipe_plan.clear();
                self.pipe_plan_key = None;
                self.pipe_status = error;
                return;
            }
        };
        let key = PipePlanKey {
            start,
            end,
            terrain: self.model.revision,
            geometry: self.model.equipment_geometry_revision,
        };
        if self.pipe_plan_key.as_ref() == Some(&key) {
            return;
        }
        self.pipe_plan_key = Some(key);
        self.pipe_plan_revision = self.pipe_plan_revision.wrapping_add(1);
        self.pipe_plan.clear();
        match game_simulation::pipe_route::plan(&self.model.terrain, start, end).and_then(|cells| {
            let ends =
                game_simulation::pipe_route::validate(&self.model.terrain, start, end, &cells)?;
            Ok(cells.into_iter().zip(ends).collect::<Vec<_>>())
        }) {
            Ok(plan) => {
                self.pipe_status = if plan.is_empty() {
                    "两个接口已相接，无需新管段。".into()
                } else {
                    format!("铺管：{} 段，右键提交；Esc / P 取消。", plan.len())
                };
                self.pipe_plan = plan;
            }
            Err(error) => self.pipe_status = error,
        }
    }

    pub fn click_pipe_route(&mut self, yaw: f32, pitch: f32) {
        if !self.pipe_mode || self.pipe_pending.is_some() {
            return;
        }
        self.update_pipe_route(yaw, pitch);
        if let Some(start) = self.pipe_start {
            let Some(end) = self.pipe_target else {
                return;
            };
            if self.pipe_plan.is_empty() {
                return;
            }
            let cells = self.pipe_plan.iter().map(|(cell, _)| *cell).collect();
            let inventory_revision = self.model.inventory.revision;
            if self.send_pipe(|request_id| EquipmentCommand::LayPipe {
                request_id,
                start,
                end,
                cells,
                yaw,
                pitch,
                inventory_revision,
            }) {
                self.pipe_status = "等待服务端确认整条管路… Esc / P 取消草稿。".into();
            }
        } else if self.send_pipe(|request_id| EquipmentCommand::BeginPipe {
            request_id,
            yaw,
            pitch,
        }) {
            self.pipe_status = "等待服务端确认起点… Esc / P 取消。".into();
        }
    }

    pub fn input(&mut self, input: PlayerInput) {
        if self.demo {
            let obstacles: Vec<_> = self
                .model
                .actors
                .iter()
                .filter(|a| a.id != self.model.player_id)
                .map(|a| a.position)
                .collect();
            step_in_grid(&mut self.model.body, input, &self.model.terrain, &obstacles);
        } else if let Some(t) = &self.transport {
            if let Err(e) = self.model.input(input, |m| t.send(m)) {
                self.status = e;
            }
        }
    }
    pub fn chat(&mut self, text: String) {
        if let Some(t) = &self.transport {
            if let Err(e) = t.send(ClientMessage::Chat3d { text }) {
                self.status = e;
            }
        }
    }
    pub fn container(&mut self, command: ContainerCommand) {
        if let Some(t) = &self.transport {
            if let Err(e) = t.send(ClientMessage::Container(command)) {
                self.model.container_error = e;
            }
        }
    }
    pub fn inventory(&mut self, command: InventoryCommand) {
        if let Some(t) = &self.transport {
            if let Err(e) = t.send(ClientMessage::Inventory(command)) {
                self.model.container_error = e;
            }
        }
    }
    pub fn container_target(&self, yaw: f32, pitch: f32) -> Option<[i32; 3]> {
        game_simulation::targeted_container(
            &self.model.terrain,
            self.model.body.position,
            yaw,
            pitch,
        )
    }
    pub fn block_target(&self, yaw: f32, pitch: f32) -> Option<[i32; 3]> {
        game_simulation::targeting::target_block(
            &self.model.terrain,
            self.model.body.position,
            yaw,
            pitch,
        )
        .map(|hit| hit.position)
    }
    pub fn edit_block(&mut self, yaw: f32, pitch: f32, place: bool) {
        if !self.model.ready || self.demo {
            return;
        }
        if let Some(hit) =
            equipment::target(&self.model.terrain, self.model.body.position, yaw, pitch)
        {
            let command = if place {
                self.selected_equipment()
                    .map(|kind| EquipmentCommand::Place {
                        target: hit.position,
                        yaw,
                        pitch,
                        kind,
                        tank_size: matches!(kind, EquipmentKind::Tank | EquipmentKind::Vessel)
                            .then_some(self.tank_size),
                        axis: if kind == EquipmentKind::PipeRack {
                            if self.equipment_axis == 2 {
                                2
                            } else {
                                0
                            }
                        } else if matches!(
                            kind,
                            EquipmentKind::Pipe
                                | EquipmentKind::Pump
                                | EquipmentKind::Valve
                                | EquipmentKind::PipeRack
                        ) {
                            self.equipment_axis
                        } else {
                            0
                        },
                        inventory_revision: self.model.inventory.revision,
                        slot: self.model.inventory.selected,
                    })
            } else {
                hit.equipment
                    .and_then(|id| self.model.terrain.equipment.get(&id))
                    .map(|device| EquipmentCommand::Remove {
                        id: device.id,
                        revision: device.revision,
                        yaw,
                        pitch,
                        inventory_revision: self.model.inventory.revision,
                    })
            };
            if let Some(command) = command {
                if let Some(t) = &self.transport {
                    if let Err(e) = t.send(ClientMessage::Equipment(command)) {
                        self.model.container_error = e;
                    }
                }
                return;
            }
        }
        let Some(hit) = game_simulation::targeting::target_block(
            &self.model.terrain,
            self.model.body.position,
            yaw,
            pitch,
        ) else {
            self.model.container_error = "未瞄准可操作的方块。".into();
            return;
        };
        let command = BlockEdit {
            target: hit.position,
            expected: self.model.terrain.get(hit.position),
            yaw,
            pitch,
            place,
            slot: self.model.inventory.selected,
            inventory_revision: self.model.inventory.revision,
        };
        if let Some(t) = &self.transport {
            if let Err(e) = t.send(ClientMessage::EditBlock(command)) {
                self.model.container_error = e;
            }
        }
    }
    pub fn selected_equipment(&self) -> Option<EquipmentKind> {
        self.model
            .inventory
            .slots
            .get(self.model.inventory.selected as usize)
            .and_then(Option::as_ref)
            .and_then(|item| EquipmentKind::from_item(&item.name))
    }

    pub fn configure_equipment(&mut self, command: EquipmentCommand) {
        if !self.model.ready || self.demo {
            return;
        }
        if let Some(transport) = &self.transport {
            if let Err(error) = transport.send(ClientMessage::Equipment(command)) {
                self.model.container_error = error;
            }
        }
    }

    pub fn equipment_target(&self, yaw: f32, pitch: f32) -> Option<&EquipmentSnapshot> {
        if !self.model.ready || self.demo {
            return None;
        }
        let hit = equipment::target(&self.model.terrain, self.model.body.position, yaw, pitch)?;
        self.model.terrain.equipment.get(&hit.equipment?)
    }

    /// Local feedback only: the server repeats all reach, occupancy and inventory checks.
    pub fn equipment_preview(
        &self,
        yaw: f32,
        pitch: f32,
    ) -> Option<(EquipmentSnapshot, &'static str)> {
        if !self.model.ready || self.demo {
            return None;
        }
        let kind = self.selected_equipment()?;
        let hit = equipment::target(&self.model.terrain, self.model.body.position, yaw, pitch)?;
        let device = EquipmentSnapshot {
            id: 1,
            revision: 0,
            kind,
            tank_size: matches!(kind, EquipmentKind::Tank | EquipmentKind::Vessel)
                .then_some(self.tank_size),
            origin: equipment::placement_origin_sized(hit, kind, Some(self.tank_size)),
            axis: if kind == EquipmentKind::PipeRack {
                if self.equipment_axis == 2 {
                    2
                } else {
                    0
                }
            } else if matches!(
                kind,
                EquipmentKind::Pipe
                    | EquipmentKind::Pump
                    | EquipmentKind::Valve
                    | EquipmentKind::PipeRack
            ) {
                self.equipment_axis
            } else {
                0
            },
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            thermal: (kind == EquipmentKind::HeatExchanger).then(ThermalSettings::default),
            pipe_ends: None,
            reading: None,
        };
        let cells = equipment::occupied_cells(&device);
        let mut reason = "";
        if !device.valid() {
            reason = "超出世界边界";
        } else if cells.iter().any(|p| self.model.terrain.get(*p) != AIR) {
            reason = "位置被占用或尚未加载";
        } else if self.model.terrain.equipment.values().any(|other| {
            if equipment::rack_compatible(&device, other) {
                return false;
            }
            equipment::occupied_cells(other)
                .iter()
                .any(|p| cells.contains(p))
        }) {
            reason = "位置已有设备";
        } else {
            let boxes = equipment::collision_boxes(&device);
            let overlaps = |p: [f32; 3]| {
                boxes.iter().any(|b| {
                    p[0] + game_simulation::RADIUS > b.min[0]
                        && p[0] - game_simulation::RADIUS < b.max[0]
                        && p[1] + game_simulation::HEIGHT > b.min[1]
                        && p[1] < b.max[1]
                        && p[2] + game_simulation::RADIUS > b.min[2]
                        && p[2] - game_simulation::RADIUS < b.max[2]
                })
            };
            if overlaps(self.model.body.position)
                || self
                    .model
                    .actors
                    .iter()
                    .any(|a| a.id != self.model.player_id && overlaps(a.position))
            {
                reason = "位置与角色重叠";
            } else if self
                .model
                .items
                .iter()
                .any(|item| cells.contains(&item.position.map(|p| p.floor() as i32)))
            {
                reason = "请先拾取地面物品";
            }
        }
        Some((device, reason))
    }
    pub fn load_demo(&mut self) {
        self.disconnect();
        if self.local_running() {
            return;
        }
        self.local_error = false;
        self.demo = true;
        self.status = "Offline 3D walkthrough / connect for combat and multiplayer".into();
        let mut grid = Grid::empty();
        for z in 0..64 {
            for x in 0..64 {
                for y in 0..=6 {
                    grid.set([x, y, z], if y == 6 { GRASS } else { DIRT });
                }
            }
        }
        for z in 18..=26 {
            for x in 23..=34 {
                grid.set([x, 6, z], PLANK);
                if x == 23 || x == 34 || z == 18 {
                    for y in 7..11 {
                        grid.set([x, y, z], STONE);
                    }
                }
                if z < 21 {
                    grid.set([x, 11, z], PLANK);
                }
            }
        }
        for i in 0..4 {
            for y in 7..=7 + i {
                grid.set([36, y, 27 - i], STONE);
            }
        }
        for (x, z) in [(20, 30), (40, 20), (42, 35), (20, 17), (15, 26), (37, 14)] {
            for y in 7..12 {
                grid.set([x, y, z], WOOD);
            }
            for y in 10..14 {
                for dx in -2..=2 {
                    for dz in -2..=2 {
                        grid.set([x + dx, y, z + dz], LEAVES);
                    }
                }
            }
        }
        self.model.apply(ServerMessage::Welcome3d {
            player_id: 1,
            seed: 0,
            rotation: [0.0, -0.1],
        });
        for origin in Grid::origins() {
            self.model.apply(ServerMessage::Chunk3d {
                origin,
                blocks: grid.chunk(origin),
            });
        }
        self.model.apply(ServerMessage::World3d(Snapshot3d {
            flying: false,
            seq: 0,
            body: Body {
                position: [28.5, 7.0, 32.5],
                grounded: true,
                ..Default::default()
            },
            actors: vec![Actor3d {
                id: 2,
                kind: 1,
                position: [31.5, 7.0, 25.5],
                yaw: 0.0,
                hp: 30,
                max_hp: 30,
            }],
            items: vec![GroundItem3d {
                id: 1,
                position: [27.0, 7.0, 29.0],
                name: "Supplies".into(),
            }],
            hp: 100,
            inventory: PlayerInventory::default(),
            log: vec![],
            container: None,
            container_error: String::new(),
        }));
    }
}

#[cfg(test)]
mod pipe_tests {
    use super::*;

    #[test]
    fn distillation_send_cancel_retry_and_late_results_obey_session_lifetime() {
        let mut session = Session::default();
        session.model.ready = true;
        session.transport = Some(Box::new(Queue { fail: true }));
        assert!(session.open_distillation());
        assert!(!session.calculate_distillation(DistillationSpec::default()));
        assert_eq!(session.distillation_request, 0);
        assert!(!session.distillation_pending);
        session.transport = Some(Box::new(Queue { fail: false }));
        assert!(session.calculate_distillation(DistillationSpec::default()));
        assert_eq!(session.distillation_active, Some(1));
        session.transport = Some(Box::new(Queue { fail: true }));
        session.close_distillation();
        assert!(!session.distillation_open && !session.distillation_pending);
        assert_eq!(session.distillation_cancel_pending, Some(1));
        assert!(session.open_distillation());
        assert!(!session.calculate_distillation(DistillationSpec::default()));
        assert_eq!(session.distillation_request, 1);
        assert!(session.distillation_active.is_none());
        session.close_distillation();
        session.accept_distillation_reply(DistillationReply {
            request_id: 1,
            status: DistillationStatus::Failed,
            message: "late".into(),
            result: None,
        });
        assert!(session.distillation_reply.is_none());
        session.transport = Some(Box::new(Queue { fail: false }));
        session.tick();
        assert!(session.distillation_cancel_pending.is_none());
        assert!(session.open_distillation());
        assert!(session.calculate_distillation(DistillationSpec::default()));
        session.accept_distillation_reply(DistillationReply {
            request_id: 1,
            status: DistillationStatus::Failed,
            message: "older request".into(),
            result: None,
        });
        assert!(session.distillation_pending && session.distillation_reply.is_none());
        session.accept_distillation_reply(DistillationReply {
            request_id: 2,
            status: DistillationStatus::Failed,
            message: "current failure".into(),
            result: None,
        });
        assert!(!session.distillation_pending);
        session.accept_distillation_reply(DistillationReply {
            request_id: 2,
            status: DistillationStatus::Running,
            message: "late running".into(),
            result: None,
        });
        assert_eq!(
            session.distillation_reply.as_ref().unwrap().status,
            DistillationStatus::Failed
        );
        session.disconnect();
        assert!(!session.distillation_open && session.distillation_reply.is_none());
        assert!(
            session.distillation_spec.is_none() && session.distillation_cancel_pending.is_none()
        );
        assert_eq!(session.distillation_request, 0);
    }

    #[test]
    fn distillation_invalid_input_and_demo_do_not_create_jobs() {
        let mut session = Session {
            transport: Some(Box::new(Queue { fail: false })),
            ..Default::default()
        };
        assert!(!session.open_distillation());
        session.model.ready = true;
        session.demo = true;
        assert!(!session.open_distillation());
        session.demo = false;
        assert!(session.open_distillation());
        let spec = DistillationSpec {
            feed_tray: 0,
            ..Default::default()
        };
        assert!(!session.calculate_distillation(spec));
        assert!(!session.distillation_pending && session.distillation_request == 0);
        session.model.hp = 0;
        assert!(!session.calculate_distillation(DistillationSpec::default()));
    }

    #[test]
    fn assembly_replies_ignore_closed_panels_old_requests_and_other_masters() {
        let report = |request_id, master_id, status| AssemblyReport {
            request_id,
            master_id,
            status,
            lines: vec![],
            message: "check".into(),
        };
        let mut session = Session {
            assembly_master: Some(3),
            assembly_request: 4,
            assembly_pending: true,
            ..Default::default()
        };
        session.accept_assembly_reply(report(3, 3, AssemblyStatus::Rejected));
        session.accept_assembly_reply(report(4, 2, AssemblyStatus::Rejected));
        assert!(session.assembly_report.is_none() && session.assembly_pending);
        session.accept_assembly_reply(report(4, 3, AssemblyStatus::Rejected));
        assert!(!session.assembly_pending);
        session.accept_assembly_reply(report(4, 3, AssemblyStatus::Stale));
        assert_eq!(
            session.assembly_report.as_ref().unwrap().status,
            AssemblyStatus::Stale
        );
        session.clear_assembly();
        session.accept_assembly_reply(report(4, 3, AssemblyStatus::Stale));
        assert!(session.assembly_report.is_none());
        session.tank_size = TankSize {
            diameter: 6,
            height: 12,
        };
        session.disconnect();
        assert_eq!(session.tank_size, TankSize::default());
        assert_eq!(session.assembly_request, 0);
    }
    struct Queue {
        fail: bool,
    }
    impl Transport for Queue {
        fn send(&self, _: ClientMessage) -> Result<(), String> {
            if self.fail {
                Err("queue full".into())
            } else {
                Ok(())
            }
        }
        fn poll(&mut self) -> Option<NetworkEvent> {
            None
        }
    }
    fn anchor() -> PipeAnchor {
        PipeAnchor {
            id: 1,
            revision: 0,
            port: 0,
        }
    }
    #[test]
    fn fresh_authority_cancel_clears_an_accepted_start_but_old_cancel_is_ignored() {
        let mut session = Session {
            pipe_request: 2,
            pipe_mode: true,
            pipe_start: Some(anchor()),
            ..Default::default()
        };
        session.pipe_plan.push(([0, 1, 0], [0, 1]));
        session.accept_pipe_reply(PipeRouteReply {
            request_id: 2,
            stage: PipeRouteStage::Cancelled,
            anchor: None,
            message: "expired".into(),
        });
        assert!(!session.pipe_mode && session.pipe_start.is_none() && session.pipe_plan.is_empty());
        assert_eq!(session.pipe_status, "expired");
        session.pipe_request = 4;
        session.pipe_pending = Some(4);
        session.pipe_mode = true;
        session.pipe_start = Some(anchor());
        session.accept_pipe_reply(PipeRouteReply {
            request_id: 2,
            stage: PipeRouteStage::Cancelled,
            anchor: None,
            message: "old".into(),
        });
        assert_eq!(session.pipe_pending, Some(4));
        assert_eq!(session.pipe_start, Some(anchor()));
        assert!(session.pipe_mode);
    }
    #[test]
    fn rejected_missing_anchor_and_disconnect_clear_route_state() {
        let mut session = Session {
            pipe_request: 2,
            pipe_pending: Some(2),
            pipe_mode: true,
            pipe_start: Some(anchor()),
            ..Default::default()
        };
        session.pipe_plan.push(([0, 1, 0], [0, 1]));
        session.accept_pipe_reply(PipeRouteReply {
            request_id: 2,
            stage: PipeRouteStage::Rejected,
            anchor: None,
            message: "changed".into(),
        });
        assert!(
            session.pipe_start.is_none()
                && session.pipe_plan.is_empty()
                && session.pipe_pending.is_none()
        );
        session.pipe_start = Some(anchor());
        session.disconnect();
        assert!(
            !session.pipe_mode && session.pipe_start.is_none() && session.pipe_pending.is_none()
        );
        assert_eq!(session.pipe_request, 0);
    }
    #[test]
    fn failed_send_does_not_enter_pending_and_requests_never_wrap() {
        let mut session = Session {
            transport: Some(Box::new(Queue { fail: true })),
            ..Default::default()
        };
        assert!(
            !session.send_pipe(|request_id| EquipmentCommand::BeginPipe {
                request_id,
                yaw: 0.0,
                pitch: 0.0
            })
        );
        assert!(session.pipe_pending.is_none());
        assert_eq!(session.pipe_request, 1);
        session.transport = Some(Box::new(Queue { fail: false }));
        assert!(session.send_pipe(|request_id| EquipmentCommand::BeginPipe {
            request_id,
            yaw: 0.0,
            pitch: 0.0
        }));
        assert_eq!(session.pipe_pending, Some(2));
        session.pipe_pending = None;
        session.pipe_request = i64::MAX as u64;
        assert!(!session.send_pipe(|request_id| EquipmentCommand::CancelPipe { request_id }));
        assert!(session.pipe_pending.is_none());
    }
    #[test]
    fn delayed_cancel_ack_cannot_exit_a_reopened_tool_before_its_next_begin() {
        let mut session = Session {
            transport: Some(Box::new(Queue { fail: false })),
            pipe_mode: true,
            ..Default::default()
        };
        session.model.ready = true;
        session.model.inventory.slots[0] = Some(InventoryEntry {
            name: PIPE_ITEM.into(),
            quantity: 4,
        });
        session.cancel_pipe_route();
        let cancelled = session.pipe_request;
        assert_eq!(session.pipe_pending, Some(cancelled));
        session.toggle_pipe_mode();
        assert!(
            session.pipe_mode && session.pipe_start.is_none() && session.pipe_pending.is_none()
        );
        session.accept_pipe_reply(PipeRouteReply {
            request_id: cancelled,
            stage: PipeRouteStage::Cancelled,
            anchor: None,
            message: "old cancel".into(),
        });
        assert!(
            session.pipe_mode && session.pipe_start.is_none() && session.pipe_pending.is_none()
        );
        assert!(session.send_pipe(|request_id| EquipmentCommand::BeginPipe {
            request_id,
            yaw: 0.0,
            pitch: 0.0
        }));
        assert_eq!(session.pipe_pending, Some(cancelled + 1));
    }
}
