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
            inventory_revision,
            slot,
        }))
        .await;
    }

    async fn point_at(&mut self, device: &EquipmentSnapshot) -> (f32, f32) {
        let point = if device.kind == EquipmentKind::Tank {
            [
                device.origin[0] as f32 + 1.0,
                device.origin[1] as f32 + 1.4,
                device.origin[2] as f32 + 1.0,
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
