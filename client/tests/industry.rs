//! Real QUIC coverage for equipment identity, streaming and atomic inventory edits.
use client::network_client::{connect_endpoint, read_message, write_message};
use game_client_core::voxel::Client3d;
use protocol::*;
use server::{
    network::{make_server_endpoint, serve_endpoint},
    voxel::VoxelWorld,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{oneshot, Mutex};

struct Peer {
    _endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    send: quinn::SendStream,
    model: Arc<Mutex<Client3d>>,
    reader: tokio::task::JoinHandle<()>,
    sequence: u32,
}

impl Peer {
    async fn select_equipment(&mut self, name: &str) {
        let slot = self
            .model
            .lock()
            .await
            .inventory
            .slots
            .iter()
            .position(|item| item.as_ref().is_some_and(|item| item.name == name))
            .unwrap() as u8;
        self.send(ClientMessage::Inventory(InventoryCommand::Select { slot }))
            .await;
        self.wait(|m| m.inventory.selected == slot).await;
    }

    async fn point_port(&mut self, anchor: PipeAnchor) -> (f32, f32) {
        let point = {
            let m = self.model.lock().await;
            game_simulation::equipment::ports(&m.terrain.equipment[&anchor.id])
                [usize::from(anchor.port)]
            .position
        };
        let (yaw, pitch) = aim(self.model.lock().await.body.position, point);
        self.sequence += 1;
        self.send(ClientMessage::Input3d {
            seq: self.sequence,
            input: PlayerInput {
                yaw,
                pitch,
                ..Default::default()
            },
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        (yaw, pitch)
    }

    async fn begin_pipe(&mut self, request_id: u64, anchor: PipeAnchor) {
        let (yaw, pitch) = self.point_port(anchor).await;
        self.send(ClientMessage::Equipment(EquipmentCommand::BeginPipe {
            request_id,
            yaw,
            pitch,
        }))
        .await;
        self.wait(|m| {
            m.pipe_reply
                .as_ref()
                .is_some_and(|reply| reply.request_id == request_id)
        })
        .await;
        assert_eq!(
            self.model.lock().await.pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Started
        );
    }

    async fn connect(address: std::net::SocketAddr, cert: Vec<u8>, identity: u8) -> Self {
        let (endpoint, connection) = connect_endpoint(address, cert).await.unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        write_message(
            &mut send,
            &ClientMessage::Login {
                version: PROTOCOL_VERSION,
                player_name: format!("Industrial {identity}"),
                player_uuid: [identity; 16],
            },
        )
        .await
        .unwrap();
        let model = Arc::new(Mutex::new(Client3d::default()));
        let received = model.clone();
        let reader = tokio::spawn(async move {
            while let Ok(message) = read_message(&mut recv).await {
                received.lock().await.apply(message);
            }
        });
        let peer = Self {
            _endpoint: endpoint,
            connection,
            send,
            model,
            reader,
            sequence: 0,
        };
        peer.wait(|m| m.ready).await;
        peer
    }

    async fn wait(&self, predicate: impl Fn(&Client3d) -> bool) {
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if predicate(&*self.model.lock().await) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if result.is_err() {
            let model = self.model.lock().await;
            panic!("industrial condition timed out: body={:?}, equipment={:?}, inventory={:?}, error={}, log={:?}",
                model.body.position, model.terrain.equipment, model.inventory, model.container_error, model.log);
        }
    }

    async fn send(&mut self, message: ClientMessage) {
        write_message(&mut self.send, &message).await.unwrap();
    }

    async fn chat(&mut self, text: &str) {
        self.send(ClientMessage::Chat3d { text: text.into() }).await;
    }

    async fn teleport(&mut self, position: [f32; 3]) {
        tokio::time::sleep(Duration::from_millis(550)).await;
        self.chat(&format!(
            "/tp {} {} {}",
            position[0], position[1], position[2]
        ))
        .await;
        self.wait(|m| {
            game_simulation::distance(m.body.position, position) < 0.01
                && m.terrain.nearby_known(position)
        })
        .await;
    }

    async fn place(&mut self, kind: EquipmentKind, axis: u8, target: [i32; 3]) {
        self.place_sized(kind, axis, target, None).await;
    }

    async fn place_sized(
        &mut self,
        kind: EquipmentKind,
        axis: u8,
        target: [i32; 3],
        tank_size: Option<TankSize>,
    ) {
        let name = kind.item_name();
        let slot = self
            .model
            .lock()
            .await
            .inventory
            .slots
            .iter()
            .position(|item| item.as_ref().is_some_and(|i| i.name == name))
            .unwrap() as u8;
        assert!(slot < HOTBAR_SLOTS as u8);
        self.send(ClientMessage::Inventory(InventoryCommand::Select { slot }))
            .await;
        self.wait(|m| m.inventory.selected == slot).await;
        let m = self.model.lock().await;
        let (yaw, pitch) = aim(
            m.body.position,
            [
                target[0] as f32 + 0.5,
                target[1] as f32 + 1.0,
                target[2] as f32 + 0.5,
            ],
        );
        let inventory_revision = m.inventory.revision;
        drop(m);
        self.send(ClientMessage::Equipment(EquipmentCommand::Place {
            target,
            yaw,
            pitch,
            kind,
            axis,
            tank_size,
            inventory_revision,
            slot,
        }))
        .await;
    }

    async fn point_at(&mut self, device: &EquipmentSnapshot) -> (f32, f32) {
        let point = if matches!(device.kind, EquipmentKind::Tank | EquipmentKind::Vessel) {
            [
                device.origin[0] as f32 + 1.0,
                device.origin[1] as f32 + 1.4,
                device.origin[2] as f32 + 1.0,
            ]
        } else if device.kind == EquipmentKind::HeatExchanger {
            [
                device.origin[0] as f32 + 1.0,
                device.origin[1] as f32 + 0.5,
                device.origin[2] as f32 + 1.5,
            ]
        } else {
            device.origin.map(|v| v as f32 + 0.5)
        };
        let (yaw, pitch) = aim(self.model.lock().await.body.position, point);
        self.sequence += 1;
        self.send(ClientMessage::Input3d {
            seq: self.sequence,
            input: PlayerInput {
                yaw,
                pitch,
                ..Default::default()
            },
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        (yaw, pitch)
    }

    async fn configure(&mut self, id: u64, enabled: bool, opening: f64, reversed: bool) {
        let device = self.model.lock().await.terrain.equipment[&id].clone();
        let (yaw, pitch) = self.point_at(&device).await;
        self.send(ClientMessage::Equipment(EquipmentCommand::Configure {
            id,
            revision: device.revision,
            yaw,
            pitch,
            enabled,
            opening,
            pump_reversed: reversed,
        }))
        .await;
        self.wait(|m| m.terrain.equipment[&id].revision > device.revision)
            .await;
    }

    async fn configure_thermal(&mut self, id: u64, settings: ThermalSettings) {
        let device = self.model.lock().await.terrain.equipment[&id].clone();
        let (yaw, pitch) = self.point_at(&device).await;
        self.send(ClientMessage::Equipment(
            EquipmentCommand::ConfigureThermal {
                id,
                revision: device.revision,
                yaw,
                pitch,
                settings,
            },
        ))
        .await;
        self.wait(|m| m.terrain.equipment[&id].revision > device.revision)
            .await;
    }

    async fn check_assembly(&mut self, id: u64, request_id: u64) -> AssemblyReport {
        let device = self.model.lock().await.terrain.equipment[&id].clone();
        let (yaw, pitch) = self.point_at(&device).await;
        self.send(ClientMessage::Equipment(EquipmentCommand::CheckAssembly {
            request_id,
            id,
            revision: device.revision,
            yaw,
            pitch,
        }))
        .await;
        self.wait(|m| {
            m.assembly_reply
                .as_ref()
                .is_some_and(|r| r.request_id == request_id)
        })
        .await;
        self.model.lock().await.assembly_reply.clone().unwrap()
    }
}

#[tokio::test]
async fn industrial_parameterized_tanks_and_assembly_emit_graphics_fixture() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let mut world = VoxelWorld::new(12345).unwrap();
        for x in [-16, 0, 16] {
            for z in [0, 16, 32] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let origin = [x, y, z];
                    if !world.terrain.blocks.contains_key(&origin) {
                        assert!(world.terrain.apply_chunk(origin, &vec![AIR; 4096]));
                    }
                }
            }
        }
        for x in -10..28 {
            for z in 10..39 {
                world.terrain.set([x, 80, z], STONE);
                for y in 81..100 {
                    world.terrain.set([x, y, z], AIR);
                }
            }
        }
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(world));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let mut first = Peer::connect(address, cert.clone(), 97).await;
        let mut second = Peer::connect(address, cert, 98).await;
        first.teleport([0.5, 81.0, 13.5]).await;
        second.teleport([16.5, 81.0, 24.5]).await;
        for count in 1..=3 {
            tokio::time::sleep(Duration::from_millis(550)).await;
            first.chat("/industry kit").await;
            first
                .wait(|m| {
                    m.inventory
                        .iter()
                        .filter(|i| i.name == TANK_ITEM)
                        .map(|i| i.quantity as usize)
                        .sum::<usize>()
                        >= count * 2
                })
                .await;
        }
        second.chat("/industry kit").await;
        second
            .wait(|m| m.inventory.iter().any(|i| i.name == TANK_ITEM))
            .await;
        let mut ids = std::collections::BTreeMap::new();
        for z in [16, 18] {
            let stand_z = if z == 16 { 13.5 } else { 21.5 };
            for (x, kind) in [
                (-5, EquipmentKind::Tank),
                (-3, EquipmentKind::Pump),
                (-2, EquipmentKind::Valve),
                (-1, EquipmentKind::Pipe),
                (2, EquipmentKind::Pipe),
                (3, EquipmentKind::Tank),
            ] {
                first.teleport([x as f32 + 0.5, 81.0, stand_z]).await;
                first.place(kind, 0, [x, 80, z]).await;
                first
                    .wait(|m| m.terrain.equipment.values().any(|d| d.origin == [x, 81, z]))
                    .await;
                let id = first
                    .model
                    .lock()
                    .await
                    .terrain
                    .equipment
                    .values()
                    .find(|d| d.origin == [x, 81, z])
                    .unwrap()
                    .id;
                ids.insert((x, z), id);
            }
        }
        first.teleport([0.5, 81.0, 13.5]).await;
        first
            .place(EquipmentKind::HeatExchanger, 0, [0, 80, 16])
            .await;
        first.wait(|m| m.terrain.equipment.len() == 13).await;
        second.wait(|m| m.terrain.equipment.len() == 13).await;
        let hx = first
            .model
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .find(|d| d.kind == EquipmentKind::HeatExchanger)
            .unwrap()
            .id;

        // Invalid dimensions are rejected before any part of the build or inventory commits.
        first.teleport([8.5, 81.0, 13.5]).await;
        first.select_equipment(TANK_ITEM).await;
        let before = first.model.lock().await.inventory.clone();
        first.model.lock().await.container_error.clear();
        first
            .place_sized(
                EquipmentKind::Tank,
                0,
                [8, 80, 16],
                Some(TankSize {
                    diameter: 7,
                    height: 6,
                }),
            )
            .await;
        first.wait(|m| !m.container_error.is_empty()).await;
        assert_eq!(first.model.lock().await.inventory, before);
        assert_eq!(manager.lock().await.terrain.equipment.len(), 13);

        // Both peers propose the same footprint. Only one full-size instance and one item are committed.
        second.teleport([10.5, 81.0, 13.5]).await;
        second.select_equipment(TANK_ITEM).await;
        first.model.lock().await.container_error.clear();
        second.model.lock().await.container_error.clear();
        let size = TankSize {
            diameter: 4,
            height: 6,
        };
        let second_before = second.model.lock().await.inventory.clone();
        tokio::join!(
            first.place_sized(EquipmentKind::Tank, 0, [8, 80, 16], Some(size)),
            second.place_sized(EquipmentKind::Tank, 0, [8, 80, 16], Some(size)),
        );
        first
            .wait(|m| {
                m.terrain.equipment.len() == 14
                    && (m.inventory.revision > before.revision || !m.container_error.is_empty())
            })
            .await;
        second
            .wait(|m| {
                m.terrain.equipment.len() == 14
                    && (m.inventory.revision > second_before.revision
                        || !m.container_error.is_empty())
            })
            .await;
        let item_count = |bag: &PlayerInventory| {
            bag.iter()
                .filter(|i| i.name == TANK_ITEM)
                .map(|i| i.quantity)
                .sum::<u32>()
        };
        let first_after = first.model.lock().await.inventory.clone();
        let second_after = second.model.lock().await.inventory.clone();
        assert_eq!(
            item_count(&before) + item_count(&second_before),
            item_count(&first_after) + item_count(&second_after) + 1
        );
        assert!((first_after == before) ^ (second_after == second_before));
        let large = first
            .model
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .find(|d| d.origin == [8, 81, 16])
            .unwrap()
            .clone();
        assert_eq!(large.tank_size, Some(size));
        assert_eq!(large.size(), [4, 6, 4]);
        assert_eq!(
            facts(&second.model.lock().await.terrain.equipment[&large.id]),
            facts(&large)
        );

        first.teleport([0.5, 81.0, 13.5]).await;
        second.teleport([1.5, 81.0, 13.5]).await;
        let report = first.check_assembly(hx, 1).await;
        let other = second.check_assembly(hx, 1).await;
        assert_eq!(report.status, AssemblyStatus::Complete);
        assert_eq!(report, other);
        assert_eq!(
            report,
            server::assembly::inspect(&manager.lock().await.terrain.equipment, hx, 1)
        );

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/industry-design-quic")
            .join(uuid::Uuid::new_v4().to_string())
            .join("world3d.bin");
        let restored = {
            let world = manager.lock().await;
            world.save(&path).unwrap();
            let restored = VoxelWorld::load(&path).unwrap();
            for (&id, expected) in &world.terrain.equipment {
                assert_eq!(restored.terrain.equipment[&id], facts(expected));
            }
            assert_eq!(restored.industry_ledger(), world.industry_ledger());
            assert_eq!(
                server::assembly::inspect(&restored.terrain.equipment, hx, 1).status,
                AssemblyStatus::Complete
            );
            restored
        };
        let pointer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/industry-design-fixture-path.txt");
        std::fs::write(pointer, path.to_string_lossy().replace('\\', "/")).unwrap();
        println!("INDUSTRY_DESIGN_FIXTURE={}", path.display());

        // A physical edit invalidates both subscribers; a fresh request diagnoses the missing line.
        first.teleport([-1.5, 81.0, 13.5]).await;
        let valve = first.model.lock().await.terrain.equipment[&ids[&(-2, 16)]].clone();
        let (yaw, pitch) = first.point_at(&valve).await;
        let inventory_revision = first.model.lock().await.inventory.revision;
        first
            .send(ClientMessage::Equipment(EquipmentCommand::Remove {
                id: valve.id,
                revision: valve.revision,
                yaw,
                pitch,
                inventory_revision,
            }))
            .await;
        first
            .wait(|m| {
                !m.terrain.equipment.contains_key(&valve.id)
                    && m.assembly_reply
                        .as_ref()
                        .is_some_and(|r| r.status == AssemblyStatus::Stale)
            })
            .await;
        second
            .wait(|m| {
                m.assembly_reply
                    .as_ref()
                    .is_some_and(|r| r.status == AssemblyStatus::Stale)
            })
            .await;
        first.teleport([0.5, 81.0, 13.5]).await;
        let broken = first.check_assembly(hx, 2).await;
        assert_eq!(broken.status, AssemblyStatus::Incomplete);
        assert!(!broken.lines[0].valid);
        drop(first);
        drop(second);
        let _ = stop.send(());
        server.await.unwrap().unwrap();

        // Restore into a new authority and read the same specification over a new QUIC session.
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(
            endpoint,
            Arc::new(Mutex::new(restored)),
            async {
                let _ = stopped.await;
            },
        ));
        let reopened = Peer::connect(address, cert, 97).await;
        reopened
            .wait(|m| m.terrain.equipment.contains_key(&large.id))
            .await;
        assert_eq!(
            facts(&reopened.model.lock().await.terrain.equipment[&large.id]),
            facts(&large)
        );
        assert!(reopened.model.lock().await.assembly_reply.is_none());
        drop(reopened);
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("industrial parameterized build and assembly scenario timed out");
}

#[tokio::test]
async fn industrial_pipe_routes_are_atomic_and_emit_graphics_fixture() {
    tokio::time::timeout(Duration::from_secs(150), async {
        let mut world = VoxelWorld::new(12345).unwrap();
        for x in [-32, -16, 0] {
            for z in [0, 16] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let origin = [x, y, z];
                    if !world.terrain.blocks.contains_key(&origin) {
                        assert!(world.terrain.apply_chunk(origin, &vec![AIR; 4096]));
                    }
                }
            }
        }
        for x in -24..12 {
            for z in 10..28 {
                world.terrain.set([x, 80, z], STONE);
                for y in 81..94 {
                    world.terrain.set([x, y, z], AIR);
                }
            }
        }
        for x in -13..-5 {
            for z in 12..22 {
                for y in 81..83 {
                    world.terrain.set([x, y, z], STONE);
                }
            }
        }
        // An actual wall forces the route away from a straight Manhattan run.
        for y in 81..86 {
            for z in 15..18 {
                world.terrain.set([-13, y, z], STONE);
            }
        }
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(world));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let mut first = Peer::connect(address, cert.clone(), 95).await;
        let mut second = Peer::connect(address, cert, 96).await;
        first.teleport([-22.0, 81.0, 13.5]).await;
        second.teleport([-22.5, 81.0, 22.5]).await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        first.chat("/industry kit").await;
        first
            .wait(|m| m.inventory.iter().any(|item| item.name == PIPE_RACK_ITEM))
            .await;
        first.place(EquipmentKind::Tank, 0, [-21, 80, 16]).await;
        first.wait(|m| m.terrain.equipment.len() == 1).await;
        first.teleport([-18.5, 81.0, 13.5]).await;
        first.place(EquipmentKind::Pump, 0, [-19, 80, 16]).await;
        first.wait(|m| m.terrain.equipment.len() == 2).await;
        first.teleport([-9.0, 83.0, 13.5]).await;
        first.place(EquipmentKind::Tank, 0, [-10, 82, 16]).await;
        first.wait(|m| m.terrain.equipment.len() == 3).await;
        let (left, pump, right) = {
            let m = first.model.lock().await;
            let at = |origin| {
                m.terrain
                    .equipment
                    .values()
                    .find(|d| d.origin == origin)
                    .unwrap()
                    .id
            };
            (at([-21, 81, 16]), at([-19, 81, 16]), at([-10, 83, 16]))
        };
        let (start, end) = {
            let m = first.model.lock().await;
            (
                PipeAnchor {
                    id: pump,
                    revision: m.terrain.equipment[&pump].revision,
                    port: 1,
                },
                PipeAnchor {
                    id: right,
                    revision: m.terrain.equipment[&right].revision,
                    port: 0,
                },
            )
        };
        let (cells, ends) = {
            let m = first.model.lock().await;
            let cells = game_simulation::pipe_route::plan(&m.terrain, start, end).unwrap();
            let ends =
                game_simulation::pipe_route::validate(&m.terrain, start, end, &cells).unwrap();
            (cells, ends)
        };
        assert!(cells.len() <= MAX_PIPE_ROUTE);
        assert!(ends.iter().any(|ends| ends[0] / 2 != ends[1] / 2));
        assert!(cells.iter().any(|cell| cell[0] < -16) && cells.iter().any(|cell| cell[0] >= -16));
        let rack_cell = cells
            .iter()
            .zip(&ends)
            .find(|(cell, ends)| cell[1] == 81 && ends.iter().all(|face| *face != 2 && *face != 3))
            .unwrap()
            .0;
        first
            .teleport([rack_cell[0] as f32 + 0.5, 81.0, 13.5])
            .await;
        first
            .place(EquipmentKind::PipeRack, 0, [rack_cell[0], 80, rack_cell[2]])
            .await;
        first.wait(|m| m.terrain.equipment.len() == 4).await;
        second.wait(|m| m.terrain.equipment.len() == 4).await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        second.chat("/industry kit").await;
        second
            .wait(|m| m.inventory.iter().any(|i| i.name == PIPE_ITEM))
            .await;
        first.select_equipment(PIPE_ITEM).await;
        second.select_equipment(PIPE_ITEM).await;
        first.teleport([-17.5, 81.0, 13.5]).await;
        second.teleport([-17.5, 81.0, 19.5]).await;
        first.begin_pipe(1, start).await;
        second.begin_pipe(1, start).await;
        first.teleport([-12.5, 83.0, 13.5]).await;
        second.teleport([-12.5, 83.0, 20.5]).await;
        let (yaw, pitch) = first.point_port(end).await;
        let before = first.model.lock().await.inventory.clone();
        let mut broken = cells.clone();
        broken.push(cells[0]);
        first
            .send(ClientMessage::Equipment(EquipmentCommand::LayPipe {
                request_id: 2,
                start,
                end,
                cells: broken,
                yaw,
                pitch,
                inventory_revision: before.revision,
            }))
            .await;
        first
            .wait(|m| {
                m.pipe_reply
                    .as_ref()
                    .is_some_and(|reply| reply.request_id == 2)
            })
            .await;
        assert_eq!(
            first.model.lock().await.pipe_reply.as_ref().unwrap().stage,
            PipeRouteStage::Rejected
        );
        assert_eq!(first.model.lock().await.inventory, before);
        assert_eq!(manager.lock().await.terrain.equipment.len(), 4);
        let first_command = EquipmentCommand::LayPipe {
            request_id: 3,
            start,
            end,
            cells: cells.clone(),
            yaw,
            pitch,
            inventory_revision: before.revision,
        };
        let (other_yaw, other_pitch) = second.point_port(end).await;
        let other_revision = second.model.lock().await.inventory.revision;
        let second_command = EquipmentCommand::LayPipe {
            request_id: 2,
            start,
            end,
            cells: cells.clone(),
            yaw: other_yaw,
            pitch: other_pitch,
            inventory_revision: other_revision,
        };
        first
            .send(ClientMessage::Equipment(first_command.clone()))
            .await;
        second
            .send(ClientMessage::Equipment(second_command.clone()))
            .await;
        first
            .wait(|m| m.pipe_reply.as_ref().is_some_and(|r| r.request_id == 3))
            .await;
        second
            .wait(|m| m.pipe_reply.as_ref().is_some_and(|r| r.request_id == 2))
            .await;
        let a = first.model.lock().await.pipe_reply.as_ref().unwrap().stage;
        let b = second.model.lock().await.pipe_reply.as_ref().unwrap().stage;
        assert!(matches!(
            (a, b),
            (PipeRouteStage::Laid, PipeRouteStage::Rejected)
                | (PipeRouteStage::Rejected, PipeRouteStage::Laid)
        ));
        first
            .wait(|m| m.terrain.equipment.len() == 4 + cells.len())
            .await;
        second
            .wait(|m| m.terrain.equipment.len() == 4 + cells.len())
            .await;
        if a == PipeRouteStage::Laid {
            first.wait(|m| m.inventory.revision > before.revision).await;
        } else {
            second.wait(|m| m.inventory.revision > other_revision).await;
        }
        let count = manager.lock().await.terrain.equipment.len();
        let before_first = first.model.lock().await.inventory.clone();
        let before_second = second.model.lock().await.inventory.clone();
        if a == PipeRouteStage::Laid {
            first.send(ClientMessage::Equipment(first_command)).await;
        } else {
            second.send(ClientMessage::Equipment(second_command)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(manager.lock().await.terrain.equipment.len(), count);
        assert_eq!(first.model.lock().await.inventory, before_first);
        assert_eq!(second.model.lock().await.inventory, before_second);
        for (&cell, &ends) in cells.iter().zip(&ends) {
            let d = first
                .model
                .lock()
                .await
                .terrain
                .equipment
                .values()
                .find(|d| d.kind == EquipmentKind::Pipe && d.origin == cell)
                .unwrap()
                .clone();
            assert_eq!(d.pipe_ends, Some(ends));
        }
        // A 50 kPa test pump must lift water through the rising, obstacle-avoiding run.
        for (id, position, liters) in [
            (left, [-22.0, 81.0, 13.5], 5000),
            (right, [-9.0, 83.0, 13.5], 1500),
        ] {
            first.teleport(position).await;
            let d = first.model.lock().await.terrain.equipment[&id].clone();
            first.point_at(&d).await;
            tokio::time::sleep(Duration::from_millis(550)).await;
            first
                .chat(&format!("/industry fill {id} {liters} 300"))
                .await;
            first.wait(|m| !m.terrain.equipment[&id].is_empty()).await;
        }
        first.teleport([-18.5, 81.0, 13.5]).await;
        first.configure(pump, true, 1.0, false).await;
        first
            .wait(|m| {
                m.terrain.equipment[&right]
                    .reading
                    .as_ref()
                    .is_some_and(|r| r.flow_m3_s > 0.0)
            })
            .await;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/industry-route-quic")
            .join(uuid::Uuid::new_v4().to_string())
            .join("world3d.bin");
        {
            let world = manager.lock().await;
            world.save(&path).unwrap();
            let restored = VoxelWorld::load(&path).unwrap();
            for (&id, d) in &world.terrain.equipment {
                assert_eq!(restored.terrain.equipment[&id], facts(d));
            }
            assert_eq!(restored.industry_ledger(), world.industry_ledger());
        }
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../target/industry-route-fixture-path.txt"),
            path.to_string_lossy().replace('\\', "/"),
        )
        .unwrap();
        println!("INDUSTRY_ROUTE_FIXTURE={}", path.display());
        drop(first);
        drop(second);
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("industrial route scenario timed out");
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"test complete");
        self.reader.abort();
    }
}

fn aim(feet: [f32; 3], point: [f32; 3]) -> (f32, f32) {
    let d = [
        point[0] - feet[0],
        point[1] - feet[1] - game_simulation::EYE_HEIGHT,
        point[2] - feet[2],
    ];
    (d[0].atan2(-d[2]), d[1].atan2(d[0].hypot(d[2])))
}

fn facts(device: &EquipmentSnapshot) -> EquipmentSnapshot {
    let mut value = device.clone();
    value.reading = None;
    value
}

#[tokio::test]
async fn industrial_equipment_streams_to_two_clients_and_survives_reopen() {
    tokio::time::timeout(Duration::from_secs(45), async {
        let mut world = VoxelWorld::new(12345).unwrap();
        // An isolated elevated construction pad crossing x=0. Preserve the natural spawn.
        for x in [-16, 0] {
            for z in [0, 16] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let origin = [x, y, z];
                    if !world.terrain.blocks.contains_key(&origin) {
                        assert!(world.terrain.apply_chunk(origin, &vec![AIR; 4096]));
                    }
                }
            }
        }
        for x in -10..10 {
            for z in 10..26 {
                world.terrain.set([x, 80, z], STONE);
                for y in 81..92 {
                    world.terrain.set([x, y, z], AIR);
                }
            }
        }
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(world));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let mut first = Peer::connect(address, cert.clone(), 81).await;
        let mut second = Peer::connect(address, cert.clone(), 82).await;
        first.teleport([-3.0, 81.0, 16.5]).await;
        second.teleport([-3.0, 81.0, 20.5]).await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        first.chat("/industry kit").await;
        first
            .wait(|m| m.inventory.iter().any(|i| i.name == TANK_ITEM))
            .await;
        first.place(EquipmentKind::Tank, 0, [-1, 80, 16]).await;
        first.wait(|m| m.terrain.equipment.len() == 1).await;
        second.wait(|m| m.terrain.equipment.len() == 1).await;
        let tank = first
            .model
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .next()
            .unwrap()
            .clone();
        assert_eq!(tank.origin, [-1, 81, 16]);
        assert_eq!(
            facts(&second.model.lock().await.terrain.equipment[&tank.id]),
            facts(&tank)
        );
        assert!(game_simulation::equipment::occupied_cells(&tank)
            .iter()
            .any(|p| p[0] == -1));
        assert!(game_simulation::equipment::occupied_cells(&tank)
            .iter()
            .any(|p| p[0] == 0));
        assert_eq!(
            first
                .model
                .lock()
                .await
                .inventory
                .iter()
                .find(|i| i.name == TANK_ITEM)
                .unwrap()
                .quantity,
            1
        );

        tokio::time::sleep(Duration::from_millis(250)).await;
        first.place(EquipmentKind::Pipe, 2, [-2, 80, 18]).await;
        first.wait(|m| m.terrain.equipment.len() == 2).await;
        second.wait(|m| m.terrain.equipment.len() == 2).await;
        let pipe = first
            .model
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .find(|e| e.kind == EquipmentKind::Pipe)
            .unwrap()
            .clone();
        assert_eq!(pipe.axis, 2);

        // A stale equipment edit must preserve both the device and its returned item.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let m = first.model.lock().await;
        let before = m.inventory.clone();
        let (yaw, pitch) = aim(m.body.position, [0.0, 82.5, 17.0]);
        drop(m);
        first
            .send(ClientMessage::Equipment(EquipmentCommand::Remove {
                id: tank.id,
                revision: tank.revision + 1,
                yaw,
                pitch,
                inventory_revision: before.revision,
            }))
            .await;
        first.wait(|m| !m.container_error.is_empty()).await;
        assert_eq!(first.model.lock().await.inventory, before);
        assert_eq!(manager.lock().await.terrain.equipment.len(), 2);

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/industry-quic")
            .join(uuid::Uuid::new_v4().to_string())
            .join("world3d.bin");
        manager.lock().await.save(&path).unwrap();
        let restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(facts(&restored.terrain.equipment[&tank.id]), facts(&tank));
        assert_eq!(facts(&restored.terrain.equipment[&pipe.id]), facts(&pipe));
        assert!(restored.terrain.has_column([-1, 1]) && restored.terrain.has_column([0, 1]));

        // Streaming leaves a cross-column instance present until all its columns leave.
        second.chat("/fly on").await;
        second.wait(|m| m.flying).await;
        second.teleport([200.5, 110.0, 200.5]).await;
        second.wait(|m| m.terrain.equipment.is_empty()).await;
        second.teleport([-3.0, 81.0, 20.5]).await;
        second.wait(|m| m.terrain.equipment.len() == 2).await;
        assert_eq!(
            facts(&second.model.lock().await.terrain.equipment[&tank.id]),
            facts(&tank)
        );

        first
            .send(ClientMessage::Equipment(EquipmentCommand::Remove {
                id: tank.id,
                revision: tank.revision,
                yaw,
                pitch,
                inventory_revision: before.revision,
            }))
            .await;
        first
            .wait(|m| !m.terrain.equipment.contains_key(&tank.id))
            .await;
        second
            .wait(|m| !m.terrain.equipment.contains_key(&tank.id))
            .await;
        assert_eq!(
            first
                .model
                .lock()
                .await
                .inventory
                .iter()
                .find(|i| i.name == TANK_ITEM)
                .unwrap()
                .quantity,
            2
        );
        assert_eq!(manager.lock().await.terrain.equipment.len(), 1);
        drop(first);
        drop(second);
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("industrial multiplayer scenario timed out");
}

#[tokio::test]
async fn industrial_flow_controls_conserve_and_emit_graphics_fixture() {
    tokio::time::timeout(Duration::from_secs(90), async {
        let mut world = VoxelWorld::new(12345).unwrap();
        for x in [-16, 0] {
            for z in [0, 16] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let origin = [x, y, z];
                    if !world.terrain.blocks.contains_key(&origin) {
                        assert!(world.terrain.apply_chunk(origin, &vec![AIR; 4096]));
                    }
                }
            }
        }
        for x in -10..10 {
            for z in 10..26 {
                world.terrain.set([x, 80, z], STONE);
                for y in 81..92 {
                    world.terrain.set([x, y, z], AIR);
                }
            }
        }
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(world));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let mut first = Peer::connect(address, cert.clone(), 91).await;
        let mut second = Peer::connect(address, cert, 92).await;
        first.teleport([-3.0, 81.0, 19.5]).await;
        second.teleport([-3.0, 81.0, 22.5]).await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        first.chat("/industry kit").await;
        first
            .wait(|m| m.inventory.iter().any(|i| i.name == PUMP_ITEM))
            .await;
        let mut ids = Vec::new();
        for (x, kind) in [
            (-4, EquipmentKind::Tank),
            (-2, EquipmentKind::Pipe),
            (-1, EquipmentKind::Pump),
            (0, EquipmentKind::Valve),
            (1, EquipmentKind::Pipe),
            (2, EquipmentKind::Tank),
        ] {
            first.teleport([x as f32 + 0.5, 81.0, 19.5]).await;
            first.place(kind, 0, [x, 80, 16]).await;
            let count = ids.len() + 1;
            first.wait(|m| m.terrain.equipment.len() == count).await;
            let id = first
                .model
                .lock()
                .await
                .terrain
                .equipment
                .values()
                .find(|e| e.origin == [x, 81, 16])
                .unwrap()
                .id;
            ids.push(id);
        }
        second.wait(|m| m.terrain.equipment.len() == 6).await;
        let (left, pump, valve, right) = (ids[0], ids[2], ids[3], ids[5]);
        for (id, x, liters) in [(left, -3.0, 3000), (right, 3.0, 4000)] {
            first.teleport([x, 81.0, 19.5]).await;
            let device = first.model.lock().await.terrain.equipment[&id].clone();
            first.point_at(&device).await;
            tokio::time::sleep(Duration::from_millis(550)).await;
            first
                .chat(&format!("/industry fill {id} {liters} 293.15"))
                .await;
            first
                .wait(|m| !m.terrain.equipment[&id].contents.is_empty())
                .await;
        }
        let initial_moles: f64 = manager
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .flat_map(|e| &e.contents)
            .map(|c| c.moles)
            .sum();
        let initial_energy: f64 = manager
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .map(server::process::stored_energy_j)
            .sum();
        first.teleport([-0.5, 81.0, 19.5]).await;
        first.configure(pump, true, 1.0, false).await;
        let start_right = manager.lock().await.terrain.equipment[&right].contents[0].moles;
        first
            .wait(|m| m.terrain.equipment[&right].contents[0].moles > start_right + 1.0)
            .await;
        second
            .wait(|m| m.terrain.equipment[&right].contents[0].moles > start_right + 1.0)
            .await;
        // Physics facts can update without making a player's control revision stale.
        let pump_revision = first.model.lock().await.terrain.equipment[&pump].revision;
        tokio::time::sleep(Duration::from_millis(450)).await;
        assert_eq!(
            first.model.lock().await.terrain.equipment[&pump].revision,
            pump_revision
        );
        first.configure(valve, false, 0.0, false).await;
        tokio::time::sleep(Duration::from_millis(350)).await;
        let stopped_moles = manager.lock().await.terrain.equipment[&right].contents[0].moles;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            manager.lock().await.terrain.equipment[&right].contents[0].moles,
            stopped_moles
        );
        // A second client cannot overwrite an intervening valve edit.
        second.teleport([0.5, 81.0, 19.5]).await;
        let device = second.model.lock().await.terrain.equipment[&valve].clone();
        let (yaw, pitch) = second.point_at(&device).await;
        second
            .send(ClientMessage::Equipment(EquipmentCommand::Configure {
                id: valve,
                revision: device.revision - 1,
                yaw,
                pitch,
                enabled: false,
                opening: 1.0,
                pump_reversed: false,
            }))
            .await;
        second.wait(|m| !m.container_error.is_empty()).await;
        assert_eq!(manager.lock().await.terrain.equipment[&valve].opening, 0.0);
        second.teleport([0.5, 81.0, 22.5]).await;
        first.configure(valve, false, 0.5, false).await;
        first
            .wait(|m| m.terrain.equipment[&right].contents[0].moles > stopped_moles + 1.0)
            .await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        first.configure(pump, false, 1.0, false).await;
        tokio::time::sleep(Duration::from_millis(350)).await;
        let stopped = manager.lock().await.terrain.equipment[&right].contents[0].moles;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            manager.lock().await.terrain.equipment[&right].contents[0].moles,
            stopped
        );
        let final_moles: f64 = manager
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .flat_map(|e| &e.contents)
            .map(|c| c.moles)
            .sum();
        assert!((initial_moles - final_moles).abs() < initial_moles * 1e-12);
        {
            let world = manager.lock().await;
            let final_energy: f64 = world
                .terrain
                .equipment
                .values()
                .map(server::process::stored_energy_j)
                .sum();
            let ledger = world.industry_ledger();
            assert!(ledger.pump_work_j > 0.0);
            assert!(
                (final_energy - initial_energy - ledger.pump_work_j).abs() < initial_energy * 1e-10
            );
            assert!(
                (final_moles - ledger.injected_moles + ledger.drained_moles).abs()
                    < initial_moles * 1e-12
            );
        }
        // A full-world checkpoint captures both endpoints and the work ledger together.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/industry-flow-quic")
            .join(uuid::Uuid::new_v4().to_string())
            .join("world3d.bin");
        {
            let world = manager.lock().await;
            world.save(&path).unwrap();
            let restored = VoxelWorld::load(&path).unwrap();
            for (&id, expected) in &world.terrain.equipment {
                let mut facts = expected.clone();
                facts.reading = None;
                assert_eq!(restored.terrain.equipment[&id], facts);
            }
            assert_eq!(restored.industry_ledger(), world.industry_ledger());
        }
        // Export a source fixture; the Godot scenario makes its own copy before opening it.
        let pointer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/industry-flow-fixture-path.txt");
        std::fs::write(&pointer, path.to_string_lossy().replace('\\', "/")).unwrap();
        println!("INDUSTRY_FLOW_FIXTURE={}", path.display());
        first.teleport([-3.0, 81.0, 19.5]).await;
        let tank = first.model.lock().await.terrain.equipment[&left].clone();
        first.point_at(&tank).await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        first.chat(&format!("/industry drain {left}")).await;
        first.wait(|m| m.terrain.equipment[&left].is_empty()).await;
        second.wait(|m| m.terrain.equipment[&left].is_empty()).await;
        {
            let world = manager.lock().await;
            let ledger = world.industry_ledger();
            let moles: f64 = world
                .terrain
                .equipment
                .values()
                .map(server::process::water_moles)
                .sum();
            let energy: f64 = world
                .terrain
                .equipment
                .values()
                .map(server::process::stored_energy_j)
                .sum();
            assert!(ledger.drained_moles > 0.0);
            assert!(
                (moles - ledger.injected_moles + ledger.drained_moles).abs()
                    < initial_moles * 1e-12
            );
            assert!(
                (energy - ledger.injected_energy_j + ledger.drained_energy_j - ledger.pump_work_j)
                    .abs()
                    < initial_energy * 1e-10
            );
        }
        drop(first);
        drop(second);
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("industrial flow scenario timed out");
}

#[tokio::test]
async fn industrial_thermal_controls_sync_and_emit_graphics_fixture() {
    tokio::time::timeout(Duration::from_secs(150), async {
        let mut world = VoxelWorld::new(12345).unwrap();
        for x in [-16, 0] {
            for z in [0, 16] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let origin = [x, y, z];
                    if !world.terrain.blocks.contains_key(&origin) {
                        assert!(world.terrain.apply_chunk(origin, &vec![AIR; 4096]));
                    }
                }
            }
        }
        for x in -10..12 {
            for z in 10..29 {
                world.terrain.set([x, 80, z], STONE);
                for y in 81..92 {
                    world.terrain.set([x, y, z], AIR);
                }
            }
        }
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(world));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let mut first = Peer::connect(address, cert.clone(), 93).await;
        let mut second = Peer::connect(address, cert, 94).await;
        first.teleport([0.5, 81.0, 13.5]).await;
        second.teleport([8.5, 81.0, 24.5]).await;
        for _ in 0..2 {
            tokio::time::sleep(Duration::from_millis(550)).await;
            first.chat("/industry kit").await;
        }
        first
            .wait(|m| {
                m.inventory
                    .iter()
                    .any(|i| i.name == TANK_ITEM && i.quantity >= 4)
            })
            .await;
        let mut ids = std::collections::BTreeMap::new();
        for z in [16, 18] {
            let stand_z = if z == 16 { 13.5 } else { 21.5 };
            for (x, kind) in [
                (-4, EquipmentKind::Tank),
                (-2, EquipmentKind::Pump),
                (-1, EquipmentKind::Pipe),
                (2, EquipmentKind::Pipe),
                (3, EquipmentKind::Tank),
            ] {
                first.teleport([x as f32 + 0.5, 81.0, stand_z]).await;
                first.place(kind, 0, [x, 80, z]).await;
                first
                    .wait(|m| m.terrain.equipment.values().any(|e| e.origin == [x, 81, z]))
                    .await;
                let id = first
                    .model
                    .lock()
                    .await
                    .terrain
                    .equipment
                    .values()
                    .find(|e| e.origin == [x, 81, z])
                    .unwrap()
                    .id;
                ids.insert((x, z), id);
            }
        }
        first.teleport([0.5, 81.0, 13.5]).await;
        first
            .place(EquipmentKind::HeatExchanger, 0, [0, 80, 16])
            .await;
        first
            .wait(|m| {
                m.terrain
                    .equipment
                    .values()
                    .any(|e| e.kind == EquipmentKind::HeatExchanger)
            })
            .await;
        let hx = first
            .model
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .find(|e| e.kind == EquipmentKind::HeatExchanger)
            .unwrap()
            .id;
        first.teleport([-5.5, 81.0, 25.5]).await;
        first.place(EquipmentKind::Vessel, 0, [-6, 80, 22]).await;
        first.wait(|m| m.terrain.equipment.len() == 12).await;
        let vessel = first
            .model
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .find(|e| e.kind == EquipmentKind::Vessel)
            .unwrap()
            .id;
        second.wait(|m| m.terrain.equipment.len() == 12).await;
        for (z, temperature) in [(16, 350.0), (18, 295.0)] {
            let stand_z = if z == 16 { 13.5 } else { 21.5 };
            for (x, liters) in [(-4, 4000), (3, 2000)] {
                first.teleport([x as f32 + 1.0, 81.0, stand_z]).await;
                let id = ids[&(x, z)];
                let device = first.model.lock().await.terrain.equipment[&id].clone();
                first.point_at(&device).await;
                tokio::time::sleep(Duration::from_millis(550)).await;
                first
                    .chat(&format!("/industry fill {id} {liters} {temperature}"))
                    .await;
                first.wait(|m| !m.terrain.equipment[&id].is_empty()).await;
            }
        }
        first.teleport([-5.0, 81.0, 25.5]).await;
        let device = first.model.lock().await.terrain.equipment[&vessel].clone();
        first.point_at(&device).await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        first
            .chat(&format!("/industry charge {vessel} 1000 1000 320"))
            .await;
        first
            .wait(|m| m.terrain.equipment[&vessel].contents.len() == 2)
            .await;
        first
            .wait(|m| {
                m.terrain.equipment[&vessel]
                    .reading
                    .as_ref()
                    .and_then(|r| r.thermal.as_ref())
                    .is_some_and(|r| r.quality == ReadingQuality::Available)
            })
            .await;
        let mixture = first.model.lock().await.terrain.equipment[&vessel].clone();
        let phase = mixture.reading.as_ref().unwrap().thermal.as_ref().unwrap();
        for i in 0..2 {
            assert!((phase.liquid_moles[i] + phase.vapor_moles[i] - 1000.0).abs() < 1e-5);
        }
        assert!(phase.vapor_fraction.is_some_and(|v| v > 0.0 && v < 1.0));
        first.teleport([0.5, 81.0, 13.5]).await;
        first
            .configure_thermal(
                hx,
                ThermalSettings {
                    mode: HeatControlMode::Manual,
                    duty_fraction: 0.75,
                    target_temperature_k: 330.0,
                },
            )
            .await;
        for z in [16, 18] {
            first
                .teleport([-1.5, 81.0, if z == 16 { 13.5 } else { 21.5 }])
                .await;
            first.configure(ids[&(-2, z)], true, 1.0, false).await;
        }
        first
            .wait(|m| {
                m.terrain.equipment[&hx]
                    .reading
                    .as_ref()
                    .and_then(|r| r.thermal.as_ref())
                    .is_some_and(|r| r.heat_w.abs() > 1.0)
            })
            .await;
        second
            .wait(|m| {
                m.terrain.equipment[&hx]
                    .reading
                    .as_ref()
                    .and_then(|r| r.thermal.as_ref())
                    .is_some_and(|r| r.heat_w.abs() > 1.0)
            })
            .await;
        let sample_tick = {
            let model = first.model.lock().await;
            let reading = model.terrain.equipment[&hx]
                .reading
                .as_ref()
                .unwrap()
                .thermal
                .as_ref()
                .unwrap();
            assert_eq!(reading.quality, ReadingQuality::Available);
            assert!(reading
                .inlet_temperature_k
                .iter()
                .chain(&reading.outlet_temperature_k)
                .all(Option::is_some));
            assert!(reading.flow_m3_s.iter().all(|q| q.abs() > 1e-9));
            reading.sampled_at_tick
        };
        first
            .wait(|m| {
                m.terrain.equipment[&hx]
                    .reading
                    .as_ref()
                    .and_then(|r| r.thermal.as_ref())
                    .is_some_and(|r| r.sampled_at_tick > sample_tick)
            })
            .await;
        // Independent stream inventories remain fixed in spite of energy transfer.
        {
            let world = manager.lock().await;
            for z in [16, 18] {
                let n: f64 = [-4, 3]
                    .into_iter()
                    .map(|x| server::process::water_moles(&world.terrain.equipment[&ids[&(x, z)]]))
                    .sum();
                assert!((n - 6000.0 / 0.01801528).abs() < 1e-5);
            }
            assert_eq!(world.terrain.equipment[&vessel].contents, mixture.contents);
        }
        first.teleport([0.5, 81.0, 13.5]).await;
        first
            .configure_thermal(
                hx,
                ThermalSettings {
                    mode: HeatControlMode::OutletTemperature,
                    duty_fraction: 1.0,
                    target_temperature_k: 340.0,
                },
            )
            .await;
        let revision = first.model.lock().await.terrain.equipment[&hx].revision;
        second.teleport([1.5, 81.0, 13.5]).await;
        let device = second.model.lock().await.terrain.equipment[&hx].clone();
        let (yaw, pitch) = second.point_at(&device).await;
        second
            .send(ClientMessage::Equipment(
                EquipmentCommand::ConfigureThermal {
                    id: hx,
                    revision: revision - 1,
                    yaw,
                    pitch,
                    settings: ThermalSettings::default(),
                },
            ))
            .await;
        second.wait(|m| !m.container_error.is_empty()).await;
        assert_eq!(
            manager.lock().await.terrain.equipment[&hx]
                .thermal
                .as_ref()
                .unwrap()
                .mode,
            HeatControlMode::OutletTemperature
        );
        second.teleport([8.5, 81.0, 24.5]).await;
        for z in [16, 18] {
            first
                .teleport([-1.5, 81.0, if z == 16 { 13.5 } else { 21.5 }])
                .await;
            first.configure(ids[&(-2, z)], false, 1.0, false).await;
        }
        first
            .wait(|m| {
                m.terrain.equipment[&hx]
                    .reading
                    .as_ref()
                    .and_then(|r| r.thermal.as_ref())
                    .is_some_and(|r| r.heat_w.abs() < 1e-9)
            })
            .await;
        assert_eq!(
            first.model.lock().await.terrain.equipment[&hx]
                .reading
                .as_ref()
                .unwrap()
                .thermal
                .as_ref()
                .unwrap()
                .quality,
            ReadingQuality::NoFlow
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/industry-thermal-quic")
            .join(uuid::Uuid::new_v4().to_string())
            .join("world3d.bin");
        {
            let world = manager.lock().await;
            world.save(&path).unwrap();
            let restored = VoxelWorld::load(&path).unwrap();
            for (&id, expected) in &world.terrain.equipment {
                assert_eq!(restored.terrain.equipment[&id], facts(expected));
            }
            assert_eq!(restored.industry_ledger(), world.industry_ledger());
        }
        let pointer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/industry-thermal-fixture-path.txt");
        std::fs::write(pointer, path.to_string_lossy().replace('\\', "/")).unwrap();
        println!("INDUSTRY_THERMAL_FIXTURE={}", path.display());
        drop(first);
        drop(second);
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("industrial thermal scenario timed out");
}
