//! Real QUIC column construction, private structural reports, and restart fixture.
use exa_client::network_client::{connect_endpoint, read_message, write_message};
use exa_client_core::voxel::Client3d;
use exa_protocol::*;
use exa_server::{
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
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"column test done");
        self.reader.abort();
    }
}
impl Peer {
    async fn connect(address: std::net::SocketAddr, cert: Vec<u8>, identity: u8) -> Self {
        let (endpoint, connection) = connect_endpoint(address, cert).await.unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        write_message(
            &mut send,
            &ClientMessage::Login {
                version: PROTOCOL_VERSION,
                player_name: format!("Reflux {identity}"),
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
        };
        peer.wait(|m| m.ready).await;
        peer
    }
    async fn wait(&self, condition: impl Fn(&Client3d) -> bool) {
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if condition(&*self.model.lock().await) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if result.is_err() {
            let m = self.model.lock().await;
            panic!(
                "column condition timed out: position={:?} error={} log={:?} equipment={:?}",
                m.body.position,
                m.container_error,
                m.log,
                m.terrain.equipment.len()
            );
        }
    }
    async fn send(&mut self, message: ClientMessage) {
        write_message(&mut self.send, &message).await.unwrap();
    }
    async fn chat(&mut self, text: &str) {
        self.send(ClientMessage::Chat3d { text: text.into() }).await;
    }
    async fn teleport(&mut self, p: [f32; 3]) {
        tokio::time::sleep(Duration::from_millis(550)).await;
        self.chat(&format!("/tp {} {} {}", p[0], p[1], p[2])).await;
        self.wait(|m| {
            exa_simulation::distance(m.body.position, p) < 0.02 && m.terrain.nearby_known(p)
        })
        .await;
    }
    async fn kit(&mut self) {
        let before = self.quantity(REFLUX_COLUMN_ITEM).await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        self.chat("/industry kit").await;
        self.wait(|m| {
            m.inventory
                .iter()
                .filter(|i| i.name == REFLUX_COLUMN_ITEM)
                .map(|i| i.quantity)
                .sum::<u32>()
                > before
        })
        .await;
    }
    async fn quantity(&self, name: &str) -> u32 {
        self.model
            .lock()
            .await
            .inventory
            .iter()
            .filter(|i| i.name == name)
            .map(|i| i.quantity)
            .sum()
    }
    async fn place_command(
        &mut self,
        kind: EquipmentKind,
        origin: [i32; 3],
        design: Option<ColumnDesign>,
    ) -> ClientMessage {
        let slot = self
            .model
            .lock()
            .await
            .inventory
            .slots
            .iter()
            .position(|i| i.as_ref().is_some_and(|i| i.name == kind.item_name()))
            .unwrap() as u8;
        let slot = if slot >= 9 {
            let m = self.model.lock().await;
            let revision = m.inventory.revision;
            let quantity = m.inventory.slots[slot as usize].as_ref().unwrap().quantity;
            drop(m);
            self.send(ClientMessage::Inventory(InventoryCommand::Move {
                revision,
                from: slot,
                to: 8,
                quantity,
            }))
            .await;
            self.wait(|m| {
                m.inventory.slots[8]
                    .as_ref()
                    .is_some_and(|i| i.name == kind.item_name())
            })
            .await;
            8
        } else {
            slot
        };
        self.send(ClientMessage::Inventory(InventoryCommand::Select { slot }))
            .await;
        self.wait(|m| m.inventory.selected == slot).await;
        let m = self.model.lock().await;
        let (yaw, pitch) = aim(
            m.body.position,
            [
                origin[0] as f32 + 0.5,
                origin[1] as f32,
                origin[2] as f32 + 0.5,
            ],
        );
        ClientMessage::Equipment(EquipmentCommand::Place {
            target: [origin[0], origin[1] - 1, origin[2]],
            yaw,
            pitch,
            kind,
            axis: 0,
            tank_size: None,
            column_design: design,
            inventory_revision: m.inventory.revision,
            slot,
        })
    }
    async fn inspect(&mut self, id: u64, request_id: u64) -> AssemblyReport {
        let m = self.model.lock().await;
        let d = &m.terrain.equipment[&id];
        let (yaw, pitch) = aim(
            m.body.position,
            [
                d.origin[0] as f32 + 1.0,
                d.origin[1] as f32 + 1.4,
                d.origin[2] as f32 + 1.0,
            ],
        );
        let message = ClientMessage::Equipment(EquipmentCommand::CheckAssembly {
            request_id,
            id,
            revision: d.revision,
            yaw,
            pitch,
        });
        drop(m);
        self.send(message).await;
        self.wait(|m| {
            m.assembly_reply
                .as_ref()
                .is_some_and(|r| r.request_id == request_id)
        })
        .await;
        self.model.lock().await.assembly_reply.clone().unwrap()
    }
}
fn aim(body: [f32; 3], point: [f32; 3]) -> (f32, f32) {
    let dx = point[0] - body[0];
    let dy = point[1] - body[1] - 1.58;
    let dz = point[2] - body[2];
    (
        dx.atan2(-dz).rem_euclid(std::f32::consts::TAU),
        dy.atan2(dx.hypot(dz)),
    )
}
fn facts(d: &EquipmentSnapshot) -> EquipmentSnapshot {
    let mut d = d.clone();
    d.reading = None;
    d
}

#[path = "../../server/tests/support/reflux_fixture.rs"]
mod reflux_fixture;

fn seed_world(path: &std::path::Path) -> VoxelWorld {
    let mut world = VoxelWorld::new(12345).unwrap();
    for x in (-32..64).step_by(16) {
        for z in (-32..64).step_by(16) {
            for y in (0..WORLD_HEIGHT).step_by(16) {
                if !world.terrain.blocks.contains_key(&[x, y, z]) {
                    assert!(world.terrain.apply_chunk([x, y, z], &vec![AIR; 4096]));
                }
            }
        }
    }
    for x in -32..64 {
        for z in -32..64 {
            world.terrain.set([x, 80, z], STONE);
            for y in 81..110 {
                world.terrain.set([x, y, z], AIR);
            }
        }
    }
    let devices = reflux_fixture::fixture([0, 81, 16]);
    // Support raised vessels without filling any occupied pipe or equipment cells.
    let occupied: std::collections::BTreeSet<_> = devices
        .values()
        .flat_map(exa_simulation::equipment::occupied_cells)
        .collect();
    for device in devices
        .values()
        .filter(|d| matches!(d.kind, EquipmentKind::Vessel | EquipmentKind::Condenser))
    {
        for x in device.origin[0]..device.origin[0] + device.size()[0] {
            for z in device.origin[2]..device.origin[2] + device.size()[2] {
                let p = [x, device.origin[1] - 1, z];
                if !occupied.contains(&p) {
                    world.terrain.set(p, STONE);
                }
            }
        }
    }
    world.save(path).unwrap();
    // Seed only this fresh, isolated test archive through its public validated format.
    let archive_path = std::fs::read_dir(path.with_extension("industry"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let old = exa_server::industry_data::read(&archive_path).unwrap();
    world.terrain.equipment = devices;
    let archive = exa_server::industry_data::IndustryArchive::capture(
        &world.terrain,
        old.seed,
        old.last_update,
        old.generation,
        1,
        1000,
        old.ledger,
    )
    .unwrap();
    std::fs::remove_file(&archive_path).unwrap();
    exa_server::industry_data::write_new(&archive_path, &archive).unwrap();
    VoxelWorld::load(path).unwrap()
}

#[tokio::test]
async fn reflux_equipment_and_complete_loop_emit_graphics_fixture() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/column-loop-quic")
            .join(uuid::Uuid::new_v4().to_string())
            .join("world3d.bin");
        let world = seed_world(&path);
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(world));
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let mut a = Peer::connect(address, cert.clone(), 111).await;
        let mut b = Peer::connect(address, cert, 112).await;
        a.teleport([1.5, 81.0, 13.5]).await;
        b.teleport([2.5, 81.0, 13.5]).await;
        let report = a.inspect(1, 1).await;
        assert_eq!(report.status, AssemblyStatus::Complete, "{report:?}");
        assert_eq!(report.lines.len(), 9);
        assert!(b.model.lock().await.assembly_reply.is_none());
        assert_eq!(b.inspect(1, 1).await.status, AssemblyStatus::Complete);
        a.kit().await;
        b.kit().await;
        for (kind, z) in [
            (EquipmentKind::RefluxColumn, 0),
            (EquipmentKind::Condenser, 6),
            (EquipmentKind::Reboiler, 12),
            (EquipmentKind::Tee, 18),
        ] {
            a.teleport([40.5, 81.0, z as f32 - 2.5]).await;
            b.teleport([41.5, 81.0, z as f32 - 2.5]).await;
            let before = a.quantity(kind.item_name()).await + b.quantity(kind.item_name()).await;
            let design = kind.is_column().then(ColumnDesign::default);
            let ma = a.place_command(kind, [40, 81, z], design.clone()).await;
            let mb = b.place_command(kind, [40, 81, z], design).await;
            tokio::join!(a.send(ma.clone()), b.send(mb));
            a.wait(|m| {
                m.terrain
                    .equipment
                    .values()
                    .any(|d| d.origin == [40, 81, z] && d.kind == kind)
            })
            .await;
            b.wait(|m| {
                m.terrain
                    .equipment
                    .values()
                    .any(|d| d.origin == [40, 81, z] && d.kind == kind)
            })
            .await;
            a.wait(|m| m.inventory.revision > 0).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_eq!(
                a.quantity(kind.item_name()).await + b.quantity(kind.item_name()).await,
                before - 1
            );
            a.send(ma).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_eq!(
                a.quantity(kind.item_name()).await + b.quantity(kind.item_name()).await,
                before - 1
            );
        }
        a.teleport([1.5, 81.0, 13.5]).await;
        b.teleport([2.5, 81.0, 13.5]).await;
        assert_eq!(a.inspect(1, 2).await.status, AssemblyStatus::Complete);
        assert_eq!(b.inspect(1, 2).await.status, AssemblyStatus::Complete);
        let before: Vec<_> = manager
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .map(facts)
            .collect();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            manager
                .lock()
                .await
                .terrain
                .equipment
                .values()
                .map(facts)
                .collect::<Vec<_>>(),
            before
        );
        let restored = {
            let w = manager.lock().await;
            w.save(&path).unwrap();
            let restored = VoxelWorld::load(&path).unwrap();
            assert_eq!(
                restored
                    .terrain
                    .equipment
                    .values()
                    .map(facts)
                    .collect::<Vec<_>>(),
                before
            );
            assert_eq!(restored.industry_ledger(), w.industry_ledger());
            restored
        };
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../target/column-loop-fixture-path.txt"),
            path.to_string_lossy().replace('\\', "/"),
        )
        .unwrap();
        println!("COLUMN_LOOP_FIXTURE={}", path.display());
        // A real authoritative removal invalidates both players' cached reports.
        a.teleport([40.5, 81.0, 15.5]).await;
        let m = a.model.lock().await;
        let d = m
            .terrain
            .equipment
            .values()
            .find(|d| d.origin == [40, 81, 18])
            .unwrap();
        let id = d.id;
        let (yaw, pitch) = aim(m.body.position, [40.5, 81.5, 18.5]);
        let remove = EquipmentCommand::Remove {
            id,
            revision: d.revision,
            yaw,
            pitch,
            inventory_revision: m.inventory.revision,
        };
        drop(m);
        a.send(ClientMessage::Equipment(remove)).await;
        a.wait(|m| !m.terrain.equipment.contains_key(&id)).await;
        b.wait(|m| {
            m.assembly_reply
                .as_ref()
                .is_some_and(|r| r.status == AssemblyStatus::Stale)
        })
        .await;
        drop(a);
        drop(b);
        let _ = stop.send(());
        task.await.unwrap().unwrap();
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_endpoint(
            endpoint,
            Arc::new(Mutex::new(restored)),
            async {
                let _ = stopped.await;
            },
        ));
        let mut reopened = Peer::connect(address, cert, 111).await;
        reopened.teleport([1.5, 81.0, 13.5]).await;
        assert_eq!(
            reopened.inspect(1, 1).await.status,
            AssemblyStatus::Complete
        );
        drop(reopened);
        let _ = stop.send(());
        task.await.unwrap().unwrap();
    })
    .await
    .expect("reflux equipment scenario timed out");
}
