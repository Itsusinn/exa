//! Main-thread-only bridge. Simulation and transport remain ordinary Rust.
use client::{mesh::Mesh, movement::Movement, presentation, session::Session};
use godot::{classes::Node, prelude::*};
use protocol::PlayerInput;

struct GameExtension;
#[gdextension]
unsafe impl ExtensionLibrary for GameExtension {}

#[derive(GodotClass)]
#[class(base=Node, init)]
struct GameBridge {
    // Avoid gdext 0.5.5 generating `base: base`, flagged by Clippy 1.99 in macros.
    godot_base: Base<Node>,
    session: Session,
    movement: Movement,
    attack: bool,
    pickup: bool,
}

#[godot_api]
impl GameBridge {
    #[func]
    fn set_identity_path(&mut self, path: GString) {
        if self.session.transport.is_none() && !self.session.local_running() {
            self.session.identity_path = path.to_string().into();
        }
    }
    #[func]
    fn start_singleplayer(&mut self, path: GString, name: GString) {
        self.session.name = name.to_string();
        self.session.start_singleplayer(path.to_string().into());
        self.reset_motion();
    }

    #[func]
    fn start_demo(&mut self) {
        self.session.load_demo();
        self.reset_motion();
    }

    #[func]
    fn connect_game(&mut self, address: GString, name: GString, certificate: GString) {
        self.session.address = address.to_string();
        self.session.name = name.to_string();
        self.session.certificate = certificate.to_string();
        self.session.connect();
        self.reset_motion();
    }

    #[func]
    fn disconnect_game(&mut self) {
        // Drop signals cancellation without joining a network worker on Godot's thread.
        self.session.disconnect();
        self.reset_motion();
    }

    #[func]
    fn clear_actions(&mut self) {
        self.attack = false;
        self.pickup = false;
    }

    #[func]
    fn send_chat(&mut self, text: GString) {
        self.session.chat(text.to_string());
    }

    #[func]
    fn edit_block(&mut self, yaw: f32, pitch: f32, place: bool) {
        self.session.edit_block(yaw, pitch, place);
    }

    #[func]
    fn rotate_equipment(&mut self) {
        if self.session.selected_equipment() == Some(protocol::EquipmentKind::PipeRack) {
            self.session.equipment_axis = if self.session.equipment_axis == 0 {
                2
            } else {
                0
            };
            return;
        }
        if let Some(kind) = self
            .session
            .selected_equipment()
            .filter(|kind| kind.axis_count() > 1)
        {
            self.session.equipment_axis = (self.session.equipment_axis + 1) % kind.axis_count();
        }
    }

    #[func]
    fn toggle_pipe_mode(&mut self) {
        self.session.toggle_pipe_mode();
    }

    #[func]
    fn cancel_pipe_route(&mut self) {
        self.session.cancel_pipe_route();
    }

    #[func]
    fn click_pipe_route(&mut self, yaw: f32, pitch: f32) {
        self.session.click_pipe_route(yaw, pitch);
    }

    #[func]
    fn pipe_route_state(&mut self, yaw: f32, pitch: f32) -> VarDictionary {
        self.session.update_pipe_route(yaw, pitch);
        let s = &self.session;
        let anchor = |a: Option<protocol::PipeAnchor>| -> VarDictionary {
            a.and_then(|a| s.model.terrain.equipment.get(&a.id).and_then(|d| presentation::equipment_ports(d).get(usize::from(a.port)).copied().map(|port| (a,port))))
                .map_or_else(VarDictionary::new, |(a,p)| vdict! {"id"=>a.id as i64,"revision"=>a.revision as i64,"port"=>i64::from(a.port),"position"=>vector(p.position)})
        };
        let cells: VarArray = s.pipe_plan.iter().map(|(cell,ends)| vdict! {"origin"=>Vector3i::new(cell[0],cell[1],cell[2]),"first"=>i32::from(ends[0]),"second"=>i32::from(ends[1])}.to_variant()).collect();
        let held: u32 = s
            .model
            .inventory
            .slots
            .iter()
            .flatten()
            .filter(|item| item.name == protocol::PIPE_ITEM)
            .map(|item| item.quantity)
            .sum();
        let enough = held as usize >= s.pipe_plan.len();
        let status = if !s.pipe_plan.is_empty() && !enough {
            format!(
                "管道不足：需要 {} 件，背包有 {} 件",
                s.pipe_plan.len(),
                held
            )
        } else {
            s.pipe_status.clone()
        };
        vdict! {"active"=>s.pipe_mode,"pending"=>s.pipe_pending.is_some(),"revision"=>s.pipe_plan_revision as i64,
        "start"=>&anchor(s.pipe_start),"target"=>&anchor(s.pipe_target),"cells"=>&cells,
        "length"=>s.pipe_plan.len() as i64,"elbows"=>s.pipe_plan.iter().filter(|(_,ends)|ends[0]/2!=ends[1]/2).count() as i64,
        "materials"=>s.pipe_plan.len() as i64,"held"=>held as i64,"valid"=>!s.pipe_plan.is_empty() && enough,"message"=>status}
    }

    #[func]
    #[allow(clippy::too_many_arguments)] // Explicit scalar bridge boundary, with no gameplay rules.
    fn configure_equipment(
        &mut self,
        id: i64,
        revision: i64,
        enabled: bool,
        opening: f64,
        pump_reversed: bool,
        yaw: f32,
        pitch: f32,
    ) {
        if id <= 0 || revision < 0 || !opening.is_finite() || !(0.0..=1.0).contains(&opening) {
            return;
        }
        self.session
            .configure_equipment(protocol::EquipmentCommand::Configure {
                id: id as u64,
                revision: revision as u64,
                enabled,
                opening,
                pump_reversed,
                yaw,
                pitch,
            });
    }

    #[func]
    fn equipment_state(&self) -> VarArray {
        self.session
            .model
            .terrain
            .equipment
            .values()
            .map(|device| equipment_dictionary(device).to_variant())
            .collect()
    }

    /// Local-space anchors for authored models. No Godot-side collision or port rules.
    #[func]
    fn equipment_model_layout(&self, input: VarDictionary) -> VarDictionary {
        let integer = |key: &str| input.get(key).and_then(|v| v.try_to::<i32>().ok());
        let Some(kind) = integer("kind").and_then(equipment_kind) else {
            return VarDictionary::new();
        };
        let Some(axis) = integer("axis").and_then(|v| u8::try_from(v).ok()) else {
            return VarDictionary::new();
        };
        if axis >= kind.axis_count() {
            return VarDictionary::new();
        }
        let tank_size = if matches!(
            kind,
            protocol::EquipmentKind::Tank | protocol::EquipmentKind::Vessel
        ) {
            let Some(diameter) = integer("diameter").and_then(|v| u8::try_from(v).ok()) else {
                return VarDictionary::new();
            };
            let Some(height) = integer("height").and_then(|v| u8::try_from(v).ok()) else {
                return VarDictionary::new();
            };
            let size = protocol::TankSize { diameter, height };
            if !size.valid() {
                return VarDictionary::new();
            }
            Some(size)
        } else {
            None
        };
        let column_design = if kind.is_column() {
            let Some(design) = input
                .get("column_design")
                .and_then(|v| v.try_to::<VarDictionary>().ok())
                .and_then(|v| parse_column_design(&v))
            else {
                return VarDictionary::new();
            };
            Some(design)
        } else {
            None
        };
        let mut pipe_ends = None;
        if kind == protocol::EquipmentKind::Pipe {
            if let Some(ends) = input
                .get("pipe_ends")
                .and_then(|v| v.try_to::<VarArray>().ok())
            {
                if !ends.is_empty() {
                    if ends.len() != 2 {
                        return VarDictionary::new();
                    }
                    let faces: Option<Vec<u8>> = ends
                        .iter_shared()
                        .map(|v| v.try_to::<i32>().ok().and_then(|n| u8::try_from(n).ok()))
                        .collect();
                    let Some(faces) = faces else {
                        return VarDictionary::new();
                    };
                    if faces[0] >= faces[1] || faces[1] >= 6 {
                        return VarDictionary::new();
                    }
                    pipe_ends = Some([faces[0], faces[1]]);
                }
            }
        }
        let device = protocol::EquipmentSnapshot {
            id: 0,
            revision: 0,
            kind,
            origin: [0; 3],
            axis,
            tank_size,
            column_design,
            pipe_ends,
            contents: vec![],
            internal_energy_j: 0.0,
            enabled: false,
            opening: 1.0,
            pump_reversed: false,
            thermal: None,
            reading: None,
        };
        let mut result = equipment_dictionary(&device);
        let boxes: VarArray = presentation::equipment_collision_boxes(&device)
            .iter()
            .map(|b| vdict! { "min" => vector(b.min), "max" => vector(b.max) }.to_variant())
            .collect();
        result.set("collision_boxes", &boxes);
        result.set("nozzle_length", presentation::tank_nozzle_length_m(&device));
        result
    }

    #[func]
    #[allow(clippy::too_many_arguments)]
    fn configure_thermal(
        &mut self,
        id: i64,
        revision: i64,
        mode: i32,
        duty_fraction: f64,
        target_temperature_k: f64,
        yaw: f32,
        pitch: f32,
    ) {
        let mode = match mode {
            0 => protocol::HeatControlMode::Off,
            1 => protocol::HeatControlMode::Manual,
            2 => protocol::HeatControlMode::OutletTemperature,
            _ => return,
        };
        if id <= 0
            || revision < 0
            || !duty_fraction.is_finite()
            || !(0.0..=1.0).contains(&duty_fraction)
            || !target_temperature_k.is_finite()
            || !(250.0..=600.0).contains(&target_temperature_k)
        {
            return;
        }
        self.session
            .configure_equipment(protocol::EquipmentCommand::ConfigureThermal {
                id: id as u64,
                revision: revision as u64,
                yaw,
                pitch,
                settings: protocol::ThermalSettings {
                    mode,
                    duty_fraction,
                    target_temperature_k,
                },
            });
    }

    #[func]
    fn equipment_target(&self, yaw: f32, pitch: f32) -> VarDictionary {
        self.session
            .equipment_target(yaw, pitch)
            .map_or_else(VarDictionary::new, equipment_dictionary)
    }

    #[func]
    fn equipment_preview(&self, yaw: f32, pitch: f32) -> VarDictionary {
        self.session.equipment_preview(yaw, pitch).map_or_else(
            VarDictionary::new,
            |(device, reason)| {
                let mut result = equipment_dictionary(&device);
                result.set("valid", reason.is_empty());
                result.set("reason", reason);
                result
            },
        )
    }

    #[func]
    fn tank_build_spec(&self) -> VarDictionary {
        let Some(kind) = self.session.selected_equipment().filter(|kind| {
            matches!(
                kind,
                protocol::EquipmentKind::Tank | protocol::EquipmentKind::Vessel
            )
        }) else {
            return VarDictionary::new();
        };
        let size = self.session.tank_size;
        vdict! { "name" => kind.item_name(), "diameter" => i32::from(size.diameter), "height" => i32::from(size.height) }
    }

    #[func]
    fn set_tank_build_spec(&mut self, diameter: i32, height: i32) -> bool {
        let (Ok(diameter), Ok(height)) = (u8::try_from(diameter), u8::try_from(height)) else {
            return false;
        };
        let size = protocol::TankSize { diameter, height };
        if !size.valid() || self.tank_build_spec().is_empty() {
            return false;
        }
        self.session.tank_size = size;
        true
    }

    #[func]
    fn tank_build_capacity(&self, diameter: i32, height: i32) -> f64 {
        let (Ok(diameter), Ok(height)) = (u8::try_from(diameter), u8::try_from(height)) else {
            return 0.0;
        };
        let size = protocol::TankSize { diameter, height };
        if size.valid() {
            presentation::tank_capacity_for_size(size)
        } else {
            0.0
        }
    }

    #[func]
    fn tank_geometry(&self, kind: i32, diameter: i32, height: i32) -> VarDictionary {
        let (Ok(diameter), Ok(height)) = (u8::try_from(diameter), u8::try_from(height)) else {
            return VarDictionary::new();
        };
        let size = protocol::TankSize { diameter, height };
        if !size.valid() || !matches!(kind, 0 | 4) {
            return VarDictionary::new();
        }
        geometry(presentation::equipment_shape_sized(
            if kind == 0 {
                protocol::EquipmentKind::Tank
            } else {
                protocol::EquipmentKind::Vessel
            },
            0,
            Some(size),
        ))
    }

    #[func]
    fn check_assembly(&mut self, id: i64, revision: i64, yaw: f32, pitch: f32) {
        if id > 0 && revision >= 0 {
            self.session
                .check_assembly(id as u64, revision as u64, yaw, pitch);
        }
    }

    #[func]
    fn clear_assembly(&mut self) {
        self.session.clear_assembly();
    }

    #[func]
    fn open_distillation(&mut self) -> bool {
        self.session.open_distillation()
    }

    #[func]
    fn validate_column_design(&self, input: VarDictionary) -> bool {
        parse_column_design(&input).is_some()
    }

    #[func]
    fn use_column_design(&mut self, input: VarDictionary) -> bool {
        let Some(design) = parse_column_design(&input) else {
            return false;
        };
        self.session.column_design = Some(design);
        true
    }

    #[func]
    fn column_build_design(&self) -> VarDictionary {
        self.session
            .column_design
            .as_ref()
            .map_or_else(VarDictionary::new, column_design_dictionary)
    }

    #[func]
    fn column_selected(&self) -> bool {
        self.session
            .selected_equipment()
            .is_some_and(protocol::EquipmentKind::is_column)
    }

    #[func]
    fn column_build_kind(&self) -> i32 {
        self.session
            .selected_equipment()
            .filter(|kind| kind.is_column())
            .map_or(-1, equipment_kind_id)
    }

    #[func]
    fn column_geometry(&self, kind: i32, input: VarDictionary) -> VarDictionary {
        let Some(kind) = equipment_kind(kind).filter(|kind| kind.is_column()) else {
            return VarDictionary::new();
        };
        let Some(design) = parse_column_design(&input) else {
            return VarDictionary::new();
        };
        geometry(presentation::equipment_shape_configured(
            kind,
            0,
            None,
            Some(&design),
        ))
    }

    #[func]
    fn close_distillation(&mut self) {
        self.session.close_distillation();
    }

    #[func]
    fn cancel_distillation(&mut self) {
        self.session.cancel_distillation();
    }

    #[func]
    fn distillation_defaults(&self) -> VarDictionary {
        distillation_spec_dictionary(&protocol::DistillationSpec::default())
    }

    #[func]
    fn calculate_distillation(&mut self, input: VarDictionary) -> bool {
        let spec = (|| -> Option<protocol::DistillationSpec> {
            Some(protocol::DistillationSpec {
                trays: u8::try_from(input.get("trays")?.try_to::<i32>().ok()?).ok()?,
                feed_tray: u8::try_from(input.get("feed_tray")?.try_to::<i32>().ok()?).ok()?,
                feed_mol_s: input.get("feed_mol_s")?.try_to::<f64>().ok()?,
                butane_fraction: input.get("butane_fraction")?.try_to::<f64>().ok()?,
                feed_temperature_k: input.get("feed_temperature_k")?.try_to::<f64>().ok()?,
                pressure_pa: input.get("pressure_pa")?.try_to::<f64>().ok()?,
                reflux_ratio: input.get("reflux_ratio")?.try_to::<f64>().ok()?,
                distillate_fraction: input.get("distillate_fraction")?.try_to::<f64>().ok()?,
            })
        })();
        spec.is_some_and(|spec| self.session.calculate_distillation(spec))
    }

    #[func]
    fn distillation_state(&self) -> VarDictionary {
        let mut state = vdict! {"open" => self.session.distillation_open,
        "pending" => self.session.distillation_pending,
        "local_message" => self.session.distillation_message.as_str()};
        if let Some(spec) = &self.session.distillation_spec {
            state.set("submitted_spec", &distillation_spec_dictionary(spec));
        }
        let Some(reply) = &self.session.distillation_reply else {
            return state;
        };
        state.set("request_id", reply.request_id as i64);
        state.set(
            "status",
            match reply.status {
                protocol::DistillationStatus::Running => "running",
                protocol::DistillationStatus::Completed => "completed",
                protocol::DistillationStatus::Cancelled => "cancelled",
                protocol::DistillationStatus::Rejected => "rejected",
                protocol::DistillationStatus::Failed => "failed",
            },
        );
        state.set("message", reply.message.as_str());
        if let Some(result) = &reply.result {
            let stages: VarArray = result
                .stages
                .iter()
                .map(|s| {
                    vdict! {
                        "number" => i32::from(s.number), "temperature_k" => s.temperature_k,
                        "liquid_butane_fraction" => s.liquid_butane_fraction,
                        "vapor_butane_fraction" => s.vapor_butane_fraction,
                        "liquid_mol_s" => s.liquid_mol_s, "vapor_mol_s" => s.vapor_mol_s,
                    }
                    .to_variant()
                })
                .collect();
            state.set(
                "result",
                &vdict! {"spec" => &distillation_spec_dictionary(&result.spec),
                "stages" => &stages, "distillate_mol_s" => result.distillate_mol_s,
                "bottoms_mol_s" => result.bottoms_mol_s,
                "distillate_butane_fraction" => result.distillate_butane_fraction,
                "bottoms_butane_fraction" => result.bottoms_butane_fraction,
                "condenser_temperature_k" => result.condenser_temperature_k,
                "condenser_duty_w" => result.condenser_duty_w,
                "reboiler_duty_w" => result.reboiler_duty_w,
                "max_component_residual_mol_s" => result.max_component_residual_mol_s,
                "max_energy_residual_w" => result.max_energy_residual_w,
                "max_equilibrium_residual" => result.max_equilibrium_residual,
                "iterations" => i64::from(result.iterations)},
            );
        }
        state
    }

    #[func]
    fn assembly_report(&self) -> VarDictionary {
        let Some(report) = &self.session.assembly_report else {
            return vdict! { "pending" => self.session.assembly_pending };
        };
        let lines: VarArray = report.lines.iter().map(|line| vdict! {
            "port" => i32::from(line.port), "tank_id" => line.tank_id.unwrap_or(0) as i64,
            "pump_id" => line.pump_id.unwrap_or(0) as i64, "valve_id" => line.valve_id.unwrap_or(0) as i64,
            "valid" => line.valid, "message" => line.message.as_str()
        }.to_variant()).collect();
        vdict! { "pending" => false, "request_id" => report.request_id as i64, "master_id" => report.master_id as i64,
        "status" => match report.status { protocol::AssemblyStatus::Complete => "complete", protocol::AssemblyStatus::Incomplete => "incomplete", protocol::AssemblyStatus::Stale => "stale", protocol::AssemblyStatus::Rejected => "rejected" },
        "message" => report.message.as_str(), "lines" => &lines }
    }

    #[func]
    fn equipment_geometry(&self, kind: i32, axis: i32) -> VarDictionary {
        let Some(kind) = equipment_kind(kind) else {
            return VarDictionary::new();
        };
        if !(0..i32::from(kind.axis_count())).contains(&axis) {
            return VarDictionary::new();
        }
        geometry(presentation::equipment_shape(kind, axis as u8))
    }

    #[func]
    fn pipe_geometry(&self, axis: i32, first: i32, second: i32) -> VarDictionary {
        if !(0..=2).contains(&axis)
            || !(0..6).contains(&first)
            || !(0..6).contains(&second)
            || first >= second
        {
            return VarDictionary::new();
        }
        geometry(presentation::equipment_shape_with_ends(
            protocol::EquipmentKind::Pipe,
            axis as u8,
            Some([first as u8, second as u8]),
        ))
    }

    #[func]
    fn block_target(&self, yaw: f32, pitch: f32) -> VarDictionary {
        let m = &self.session.model;
        if !m.ready || self.session.demo {
            return VarDictionary::new();
        }
        self.session
            .block_target(yaw, pitch)
            .map_or_else(VarDictionary::new, |p| {
                vdict! { "position" => Vector3i::new(p[0],p[1],p[2]),
                "name" => protocol::block_item_name(m.terrain.get(p)).unwrap_or("") }
            })
    }

    #[func]
    fn take_dirty_chunks(&mut self) -> Array<Vector3i> {
        std::mem::take(&mut self.session.model.dirty_chunks)
            .into_iter()
            .map(|p| Vector3i::new(p[0], p[1], p[2]))
            .collect()
    }

    #[func]
    fn container_target(&self, yaw: f32, pitch: f32) -> VarDictionary {
        let m = &self.session.model;
        if self.session.demo || !m.ready {
            return VarDictionary::new();
        }
        self.session.container_target(yaw, pitch).map_or_else(
            VarDictionary::new,
            |p| vdict! { "position" => Vector3i::new(p[0],p[1],p[2]) },
        )
    }

    #[func]
    fn open_container(&mut self, position: Vector3i) {
        self.session.container(protocol::ContainerCommand::Open {
            position: [position.x, position.y, position.z],
        });
    }

    #[func]
    fn close_container(&mut self) {
        self.session.container(protocol::ContainerCommand::Close);
    }

    #[func]
    fn transfer_container(&mut self, request: VarDictionary) {
        let get = |key: &str| request.get(key).and_then(|v| v.try_to::<i64>().ok());
        let Some(position) = request
            .get("position")
            .and_then(|v| v.try_to::<Vector3i>().ok())
        else {
            return;
        };
        let (Some(revision), Some(inventory_revision), Some(slot), Some(quantity)) = (
            get("revision"),
            get("inventory_revision"),
            get("slot"),
            get("quantity"),
        ) else {
            return;
        };
        let (Ok(revision), Ok(inventory_revision), Ok(slot), Ok(quantity)) = (
            u64::try_from(revision),
            u64::try_from(inventory_revision),
            u8::try_from(slot),
            u32::try_from(quantity),
        ) else {
            return;
        };
        let target = match get("target") {
            Some(-1) | None => None,
            Some(i) => {
                let Ok(i) = u8::try_from(i) else {
                    return;
                };
                Some(i)
            }
        };
        let Some(deposit) = request.get("deposit").and_then(|v| v.try_to::<bool>().ok()) else {
            return;
        };
        self.session
            .container(protocol::ContainerCommand::Transfer {
                position: [position.x, position.y, position.z],
                revision,
                inventory_revision,
                slot,
                target,
                quantity,
                deposit,
            });
    }

    #[func]
    fn select_hotbar(&mut self, slot: i64) {
        if let Ok(slot) = u8::try_from(slot) {
            self.session
                .inventory(protocol::InventoryCommand::Select { slot });
        }
    }

    #[func]
    fn move_inventory(&mut self, revision: i64, from: i64, to: i64, quantity: i64) {
        if let (Ok(revision), Ok(from), Ok(to), Ok(quantity)) = (
            u64::try_from(revision),
            u8::try_from(from),
            u8::try_from(to),
            u32::try_from(quantity),
        ) {
            self.session.inventory(protocol::InventoryCommand::Move {
                revision,
                from,
                to,
                quantity,
            });
        }
    }

    #[func]
    fn quick_move_inventory(&mut self, revision: i64, slot: i64) {
        if let (Ok(revision), Ok(slot)) = (u64::try_from(revision), u8::try_from(slot)) {
            self.session
                .inventory(protocol::InventoryCommand::QuickMove { revision, slot });
        }
    }

    #[func]
    fn advance(&mut self, delta: f64, controls: VarDictionary) -> VarDictionary {
        let was_ready = self.session.model.ready;
        self.session.tick();
        if !was_ready && self.session.model.ready {
            // Give the host one frame to adopt the restored camera before it
            // submits the first input of the new connection.
            self.movement.sync(&self.session);
            return self.snapshot();
        }
        let number = |key: &str| {
            controls
                .get(key)
                .and_then(|v| v.try_to::<f64>().ok())
                .unwrap_or(0.0) as f32
        };
        let flag = |key: &str| {
            controls
                .get(key)
                .and_then(|v| v.try_to::<bool>().ok())
                .unwrap_or(false)
        };
        let mut input = PlayerInput {
            forward: number("forward"),
            strafe: number("strafe"),
            yaw: number("yaw"),
            pitch: number("pitch"),
            jump: flag("jump"),
            descend: flag("descend"),
            sprint: flag("sprint"),
            attack: false,
            pickup: false,
        };
        if !input.valid() || !delta.is_finite() || delta < 0.0 {
            self.session.status = "Invalid frame input".into();
            return self.snapshot();
        }
        self.attack |= flag("attack");
        self.pickup |= flag("pickup");
        let attack = &mut self.attack;
        let pickup = &mut self.pickup;
        self.movement.advance(&mut self.session, delta as f32, || {
            input.attack = std::mem::take(attack);
            input.pickup = std::mem::take(pickup);
            input
        });
        self.snapshot()
    }

    #[func]
    fn snapshot(&self) -> VarDictionary {
        let m = &self.session.model;
        let actors: VarArray = m
            .actors
            .iter()
            .filter(|a| a.id != m.player_id)
            .map(|a| {
                vdict! { "id" => i64::from(a.id), "kind" => i64::from(a.kind),
                "position" => vector(a.position), "yaw" => a.yaw,
                "hp" => a.hp, "max_hp" => a.max_hp }
                .to_variant()
            })
            .collect();
        let items: VarArray = m.items.iter().map(|i| {
            vdict! { "id" => i64::from(i.id), "position" => vector(i.position), "name" => i.name.as_str() }.to_variant()
        }).collect();
        let inventory: VarArray = m
            .inventory
            .iter()
            .map(|i| {
                vdict! { "name" => i.name.as_str(), "quantity" => i64::from(i.quantity) }
                    .to_variant()
            })
            .collect();
        let journal: VarArray = m.log.iter().map(|e| e.text.to_variant()).collect();
        let inventory_slots: VarArray = m
            .inventory
            .slots
            .iter()
            .map(|slot| {
                slot.as_ref().map_or_else(
                    || VarDictionary::new().to_variant(),
                    |i| {
                        vdict! { "name" => i.name.as_str(), "quantity" => i64::from(i.quantity) }
                            .to_variant()
                    },
                )
            })
            .collect();
        let container = m.container.as_ref().map_or_else(VarDictionary::new, |c| {
            let items: VarArray = c.items.iter().map(|i| vdict! {
                "name" => i.name.as_str(), "quantity" => i64::from(i.quantity)
            }.to_variant()).collect();
            vdict! { "position" => Vector3i::new(c.position[0],c.position[1],c.position[2]),
                "revision" => c.revision as i64, "items" => &items, "capacity" => protocol::CONTAINER_SLOTS as i64 }
        });
        vdict! {
            "ready" => m.ready, "demo" => self.session.demo,
            "local_running" => self.session.local_running(), "local_error" => self.session.local_error,
            "shutdown_pending" => self.session.shutdown_pending(),
            "connected" => self.session.transport.is_some(), "status" => self.session.status.as_str(),
            "generation" => self.session.generation as i64, "revision" => m.revision as i64,
            "equipment_revision" => m.equipment_revision as i64,
            "equipment_count" => m.terrain.equipment.len() as i64,
            "equipment_axis" => i64::from(self.session.equipment_axis),
            "loaded_sections" => m.terrain.blocks.len() as i64,
            "position" => vector(self.movement.position()), "body_position" => vector(m.body.position),
            "grounded" => m.body.grounded, "hp" => m.hp, "player_id" => i64::from(m.player_id),
            "flying" => m.flying,
            "pending" => m.pending.len() as i64,
            "spawn_rotation" => Vector2::new(m.spawn_rotation[0], m.spawn_rotation[1]),
            "inventory_slots" => &inventory_slots, "inventory_revision" => m.inventory.revision as i64,
            "selected_slot" => i64::from(m.inventory.selected), "stack_limit" => protocol::ITEM_STACK_LIMIT,
            "container" => &container, "container_error" => m.container_error.as_str(),
            "actors" => &actors, "items" => &items, "inventory" => &inventory, "journal" => &journal
        }
    }

    #[func]
    fn has_chunk(&self, origin: Vector3i) -> bool {
        self.session
            .model
            .terrain
            .blocks
            .contains_key(&[origin.x, origin.y, origin.z])
    }

    #[func]
    fn chunk_geometry(&self, origin: Vector3i) -> VarDictionary {
        let origin = [origin.x, origin.y, origin.z];
        if !protocol::valid_voxel(origin)
            || origin.iter().any(|v| v.rem_euclid(16) != 0)
            || !self.session.model.terrain.blocks.contains_key(&origin)
        {
            return VarDictionary::new();
        }
        geometry(presentation::terrain_chunk(&self.session.model, origin))
    }

    #[func]
    fn actor_geometry(&self, kind: i32) -> VarDictionary {
        let model = game_model_for_actor(kind);
        geometry(presentation::actors(&model))
    }

    #[func]
    fn item_geometry(&self) -> VarDictionary {
        let mut model = client::session::Model::default();
        model.items.push(protocol::GroundItem3d {
            id: 1,
            position: [0.0; 3],
            name: String::new(),
        });
        geometry(presentation::actors(&model))
    }

    fn reset_motion(&mut self) {
        self.movement = Movement::default();
        self.movement.sync(&self.session);
        self.attack = false;
        self.pickup = false;
    }
}

fn distillation_spec_dictionary(spec: &protocol::DistillationSpec) -> VarDictionary {
    vdict! { "trays" => i32::from(spec.trays), "feed_tray" => i32::from(spec.feed_tray),
    "feed_mol_s" => spec.feed_mol_s, "butane_fraction" => spec.butane_fraction,
    "feed_temperature_k" => spec.feed_temperature_k, "pressure_pa" => spec.pressure_pa,
    "reflux_ratio" => spec.reflux_ratio, "distillate_fraction" => spec.distillate_fraction }
}

fn column_design_dictionary(design: &protocol::ColumnDesign) -> VarDictionary {
    vdict! {"diameter" => i32::from(design.diameter), "thermo_revision" => design.thermo_revision.as_str(), "spec" => &distillation_spec_dictionary(&design.spec)}
}

fn parse_column_design(input: &VarDictionary) -> Option<protocol::ColumnDesign> {
    if input.len() != 3 {
        return None;
    }
    let diameter = column_number(input.get("diameter")?)?;
    let spec = input.get("spec")?.try_to::<VarDictionary>().ok()?;
    if spec.len() != 8 || diameter.fract() != 0.0 || !(2.0..=6.0).contains(&diameter) {
        return None;
    }
    let number = |key: &str| column_number(spec.get(key)?);
    let trays = number("trays")?;
    let feed = number("feed_tray")?;
    if trays.fract() != 0.0
        || feed.fract() != 0.0
        || !(2.0..=12.0).contains(&trays)
        || !(1.0..=trays).contains(&feed)
    {
        return None;
    }
    let design = protocol::ColumnDesign {
        diameter: diameter as u8,
        thermo_revision: input
            .get("thermo_revision")?
            .try_to::<GString>()
            .ok()?
            .to_string(),
        spec: protocol::DistillationSpec {
            trays: trays as u8,
            feed_tray: feed as u8,
            feed_mol_s: number("feed_mol_s")?,
            butane_fraction: number("butane_fraction")?,
            feed_temperature_k: number("feed_temperature_k")?,
            pressure_pa: number("pressure_pa")?,
            reflux_ratio: number("reflux_ratio")?,
            distillate_fraction: number("distillate_fraction")?,
        },
    };
    design.valid().then_some(design)
}

fn column_number(value: Variant) -> Option<f64> {
    value
        .try_to::<f64>()
        .ok()
        .or_else(|| value.try_to::<i64>().ok().map(|n| n as f64))
        .filter(|n| n.is_finite())
}

fn equipment_kind(id: i32) -> Option<protocol::EquipmentKind> {
    use protocol::EquipmentKind::*;
    Some(match id {
        0 => Tank,
        1 => Pipe,
        2 => Pump,
        3 => Valve,
        4 => Vessel,
        5 => HeatExchanger,
        6 => PipeRack,
        7 => Column,
        8 => RefluxColumn,
        9 => Condenser,
        10 => Reboiler,
        11 => Tee,
        _ => return None,
    })
}

fn equipment_kind_id(kind: protocol::EquipmentKind) -> i32 {
    use protocol::EquipmentKind::*;
    match kind {
        Tank => 0,
        Pipe => 1,
        Pump => 2,
        Valve => 3,
        Vessel => 4,
        HeatExchanger => 5,
        PipeRack => 6,
        Column => 7,
        RefluxColumn => 8,
        Condenser => 9,
        Reboiler => 10,
        Tee => 11,
    }
}

fn port_role(kind: protocol::EquipmentKind, id: u8) -> &'static str {
    use protocol::EquipmentKind::*;
    let roles: &[&str] = match kind {
        Column => &["进料", "塔顶产品", "塔底产品"],
        RefluxColumn => &["进料", "塔顶蒸气", "塔底液", "回流入口", "再沸蒸气返回"],
        Condenser => &["蒸气入口", "冷凝液出口", "冷却介质入口", "冷却介质出口"],
        Reboiler => &[
            "塔底液入口",
            "塔底产品",
            "返塔蒸气出口",
            "热媒入口",
            "热媒出口",
        ],
        Tee => &["主管负面", "主管正面", "支口"],
        _ => &[],
    };
    roles.get(usize::from(id)).copied().unwrap_or("")
}

fn equipment_dictionary(device: &protocol::EquipmentSnapshot) -> VarDictionary {
    let pipe_ends: VarArray = device
        .pipe_ends
        .into_iter()
        .flatten()
        .map(|face| i32::from(face).to_variant())
        .collect();
    let ports: VarArray = presentation::equipment_ports(device).iter().map(|port| {
        vdict! { "id" => i64::from(port.id), "role" => port_role(device.kind,port.id), "circuit" => i64::from(port.circuit), "position" => vector(port.position), "normal" => Vector3i::new(port.normal[0], port.normal[1], port.normal[2]) }.to_variant()
    }).collect();
    let contents: VarArray = device
        .contents
        .iter()
        .map(|item| {
            vdict! { "material" => item.material.as_str(), "moles" => item.moles }.to_variant()
        })
        .collect();
    vdict! { "id" => device.id as i64, "revision" => device.revision as i64,
    "name" => device.kind.item_name(), "kind" => equipment_kind_id(device.kind), "planning" => device.kind.is_planning(),
    "origin" => Vector3i::new(device.origin[0],device.origin[1],device.origin[2]), "axis" => i64::from(device.axis),
    "ports" => &ports, "pipe_ends" => &pipe_ends, "contents" => &contents, "internal_energy_j" => device.internal_energy_j,
    "diameter" => device.size()[0], "height" => device.size()[1], "column_design" => &device.column_design.as_ref().map_or_else(VarDictionary::new,column_design_dictionary),
    "design_capacity_m3" => presentation::tank_capacity_m3(device),
    "empty" => device.is_empty(), "enabled" => device.enabled, "opening" => device.opening,
    "pump_reversed" => device.pump_reversed, "thermal" => &thermal_settings_dictionary(device.thermal.as_ref()), "reading" => &reading_dictionary(device.reading.as_ref()) }
}

fn thermal_settings_dictionary(settings: Option<&protocol::ThermalSettings>) -> VarDictionary {
    settings.map_or_else(VarDictionary::new, |settings| vdict! {
        "mode" => match settings.mode { protocol::HeatControlMode::Off => 0i32, protocol::HeatControlMode::Manual => 1, protocol::HeatControlMode::OutletTemperature => 2 },
        "duty_fraction" => settings.duty_fraction, "target_temperature_k" => settings.target_temperature_k,
    })
}

fn thermal_reading_dictionary(reading: Option<&protocol::ThermalReading>) -> VarDictionary {
    reading.map_or_else(VarDictionary::new, |reading| {
        let temperatures = |values: [Option<f64>; 2]| -> VarArray {
            values.into_iter().map(|v| v.map_or(Variant::nil(), |v| v.to_variant())).collect()
        };
        let numbers = |values: [f64; 2]| -> VarArray { values.into_iter().map(|v| v.to_variant()).collect() };
        vdict! {
            "vapor_fraction" => &reading.vapor_fraction.map_or(Variant::nil(), |v| v.to_variant()),
            "liquid_moles" => &numbers(reading.liquid_moles), "vapor_moles" => &numbers(reading.vapor_moles),
            "heat_w" => reading.heat_w, "inlet_temperature_k" => &temperatures(reading.inlet_temperature_k),
            "outlet_temperature_k" => &temperatures(reading.outlet_temperature_k), "flow_m3_s" => &numbers(reading.flow_m3_s),
            "quality" => match reading.quality { protocol::ReadingQuality::Available => "available", protocol::ReadingQuality::NoFlow => "no_flow", protocol::ReadingQuality::Unavailable => "unavailable" },
            "control_state" => match reading.control_state { protocol::ControlState::Off => "off", protocol::ControlState::Manual => "manual", protocol::ControlState::Regulating => "regulating", protocol::ControlState::Limited => "limited" },
            "sampled_at_tick" => reading.sampled_at_tick as i64,
        }
    })
}

fn reading_dictionary(reading: Option<&protocol::EquipmentReading>) -> VarDictionary {
    reading.map_or_else(VarDictionary::new, |reading| vdict! {
        "temperature_k" => &reading.temperature_k.map_or(Variant::nil(), |value| value.to_variant()),
        "pressure_pa" => &reading.pressure_pa.map_or(Variant::nil(), |value| value.to_variant()), "volume_m3" => reading.volume_m3,
        "capacity_m3" => reading.capacity_m3, "flow_m3_s" => reading.flow_m3_s,
        "status" => reading.status.as_str(),
        "thermal" => &thermal_reading_dictionary(reading.thermal.as_ref()),
    })
}

fn vector(p: [f32; 3]) -> Vector3 {
    Vector3::new(p[0], p[1], p[2])
}

fn game_model_for_actor(kind: i32) -> client::session::Model {
    let mut m = client::session::Model::default();
    m.actors.push(protocol::Actor3d {
        id: 1,
        kind: u8::from(kind != 0),
        position: [0.0; 3],
        yaw: 0.0,
        hp: 100,
        max_hp: 100,
    });
    m
}

fn geometry(mesh: Mesh) -> VarDictionary {
    let mut vertices = PackedVector3Array::new();
    let mut normals = PackedVector3Array::new();
    let mut colors = PackedColorArray::new();
    let mut uv = PackedVector2Array::new();
    let mut patterns = PackedVector2Array::new();
    let len = mesh.vertices.len();
    vertices.resize(len);
    normals.resize(len);
    colors.resize(len);
    uv.resize(len);
    patterns.resize(len);
    // One allocation and mutable-slice acquisition per attribute, not five
    // engine calls per vertex. All Godot storage remains on the main thread.
    let vertex_data = vertices.as_mut_slice();
    let normal_data = normals.as_mut_slice();
    let color_data = colors.as_mut_slice();
    let uv_data = uv.as_mut_slice();
    let pattern_data = patterns.as_mut_slice();
    // Convert the CPU mesh's counter-clockwise faces to Godot's clockwise winding.
    for (triangle, tri) in mesh.vertices.as_chunks::<3>().0.iter().enumerate() {
        for (corner, i) in [0, 2, 1].into_iter().enumerate() {
            let v = tri[i];
            let index = triangle * 3 + corner;
            vertex_data[index] = vector(v.position);
            normal_data[index] = vector(v.normal);
            color_data[index] = Color::from_rgb(v.color[0], v.color[1], v.color[2]);
            uv_data[index] = Vector2::new(v.uv[0], v.uv[1]);
            pattern_data[index] = Vector2::new(
                v.pattern as f32,
                f32::from(v.face_occlusion) + 256.0 * f32::from(v.bevel_mask),
            );
        }
    }
    vdict! { "vertices" => &vertices, "normals" => &normals, "colors" => &colors, "uv" => &uv, "uv2" => &patterns }
}
