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
    base: Base<Node>,
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
        if self
            .session
            .selected_equipment()
            .is_some_and(|kind| kind != protocol::EquipmentKind::Tank)
        {
            self.session.equipment_axis = (self.session.equipment_axis + 1) % 3;
        }
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
    fn equipment_geometry(&self, kind: i32, axis: i32) -> VarDictionary {
        if !(0..=3).contains(&kind) || !(0..=2).contains(&axis) {
            return VarDictionary::new();
        }
        geometry(presentation::equipment_shape(
            match kind {
                0 => protocol::EquipmentKind::Tank,
                1 => protocol::EquipmentKind::Pipe,
                2 => protocol::EquipmentKind::Pump,
                _ => protocol::EquipmentKind::Valve,
            },
            axis as u8,
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

fn equipment_dictionary(device: &protocol::EquipmentSnapshot) -> VarDictionary {
    let ports: VarArray = presentation::equipment_ports(device).iter().map(|port| {
        vdict! { "position" => vector(port.position), "normal" => Vector3i::new(port.normal[0], port.normal[1], port.normal[2]) }.to_variant()
    }).collect();
    let contents: VarArray = device
        .contents
        .iter()
        .map(|item| {
            vdict! { "material" => item.material.as_str(), "moles" => item.moles }.to_variant()
        })
        .collect();
    vdict! { "id" => device.id as i64, "revision" => device.revision as i64,
    "name" => device.kind.item_name(), "kind" => match device.kind { protocol::EquipmentKind::Tank => 0i32, protocol::EquipmentKind::Pipe => 1, protocol::EquipmentKind::Pump => 2, protocol::EquipmentKind::Valve => 3 },
    "origin" => Vector3i::new(device.origin[0],device.origin[1],device.origin[2]), "axis" => i64::from(device.axis),
    "ports" => &ports, "contents" => &contents, "internal_energy_j" => device.internal_energy_j,
    "empty" => device.is_empty(), "enabled" => device.enabled, "opening" => device.opening,
    "pump_reversed" => device.pump_reversed, "reading" => &reading_dictionary(device.reading.as_ref()) }
}

fn reading_dictionary(reading: Option<&protocol::EquipmentReading>) -> VarDictionary {
    reading.map_or_else(VarDictionary::new, |reading| vdict! {
        "temperature_k" => &reading.temperature_k.map_or(Variant::nil(), |value| value.to_variant()),
        "pressure_pa" => &reading.pressure_pa.map_or(Variant::nil(), |value| value.to_variant()), "volume_m3" => reading.volume_m3,
        "capacity_m3" => reading.capacity_m3, "flow_m3_s" => reading.flow_m3_s,
        "status" => reading.status.as_str(),
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
            pattern_data[index] = Vector2::new(v.pattern as f32, 0.0);
        }
    }
    vdict! { "vertices" => &vertices, "normals" => &normals, "colors" => &colors, "uv" => &uv, "uv2" => &patterns }
}
