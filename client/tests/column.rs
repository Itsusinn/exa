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
                player_name: format!("Column {identity}"),
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
                m.body.position, m.container_error, m.log, m.terrain.equipment
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
        let before = self.quantity(COLUMN_ITEM).await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        self.chat("/industry kit").await;
        self.wait(|m| {
            m.inventory
                .iter()
                .filter(|i| i.name == COLUMN_ITEM)
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
        assert!(slot < 9);
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
    async fn build(&mut self, kind: EquipmentKind, origin: [i32; 3]) -> u64 {
        self.teleport([origin[0] as f32 + 0.5, origin[1] as f32, 13.5])
            .await;
        let message = self.place_command(kind, origin, None).await;
        self.send(message).await;
        self.wait(|m| m.terrain.equipment.values().any(|e| e.origin == origin))
            .await;
        self.model
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .find(|e| e.origin == origin)
            .unwrap()
            .id
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

#[tokio::test]
async fn column_construction_and_external_connections_emit_graphics_fixture() {
    tokio::time::timeout(Duration::from_secs(100), async {
        let mut world = VoxelWorld::new(12345).unwrap();
        for x in [-16, 0, 16] {
            for z in [0, 16, 32] {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    if !world.terrain.blocks.contains_key(&[x, y, z]) {
                        assert!(world.terrain.apply_chunk([x, y, z], &vec![AIR; 4096]));
                    }
                }
            }
        }
        for x in -10..28 {
            for z in 10..39 {
                world.terrain.set([x, 80, z], STONE);
                for y in 81..110 {
                    world.terrain.set([x, y, z], AIR);
                }
            }
        }
        for (range, y) in [(-5..0, 83), (2..6, 85)] {
            for x in range {
                world.terrain.set([x, y, 16], STONE);
            }
        }
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(world));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let mut a = Peer::connect(address, cert.clone(), 101).await;
        let mut b = Peer::connect(address, cert, 102).await;
        a.teleport([0.5, 81.0, 13.5]).await;
        b.teleport([1.5, 81.0, 13.5]).await;
        a.kit().await;
        a.kit().await;
        b.kit().await;
        let design = ColumnDesign {
            spec: DistillationSpec {
                trays: 4,
                feed_tray: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let before = a.quantity(COLUMN_ITEM).await;
        let bad = a
            .place_command(EquipmentKind::Column, [0, 81, 16], None)
            .await;
        a.send(bad).await;
        a.wait(|m| !m.container_error.is_empty()).await;
        assert_eq!(a.quantity(COLUMN_ITEM).await, before);
        assert!(manager.lock().await.terrain.equipment.is_empty());
        let second_before = b.quantity(COLUMN_ITEM).await;
        let ma = a
            .place_command(EquipmentKind::Column, [0, 81, 16], Some(design.clone()))
            .await;
        let mb = b
            .place_command(EquipmentKind::Column, [0, 81, 16], Some(design.clone()))
            .await;
        tokio::join!(a.send(ma.clone()), b.send(mb));
        a.wait(|m| {
            m.terrain
                .equipment
                .values()
                .any(|e| e.kind == EquipmentKind::Column)
        })
        .await;
        b.wait(|m| {
            m.terrain
                .equipment
                .values()
                .any(|e| e.kind == EquipmentKind::Column)
        })
        .await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            a.quantity(COLUMN_ITEM).await + b.quantity(COLUMN_ITEM).await,
            before + second_before - 1
        );
        a.send(ma).await; // A duplicate cannot place or charge a second time.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            a.quantity(COLUMN_ITEM).await + b.quantity(COLUMN_ITEM).await,
            before + second_before - 1
        );
        let column = a
            .model
            .lock()
            .await
            .terrain
            .equipment
            .values()
            .find(|e| e.kind == EquipmentKind::Column)
            .unwrap()
            .clone();
        assert_eq!(column.column_design, Some(design.clone()));
        assert_eq!(column.size(), [2, 6, 2]);
        assert_eq!(
            facts(&b.model.lock().await.terrain.equipment[&column.id]),
            facts(&column)
        );
        tokio::time::sleep(Duration::from_millis(550)).await;
        a.chat("/fly on").await;
        a.wait(|m| m.flying).await;
        let mut feed = 0;
        let mut breakable = 0;
        for (kind, p) in [
            (EquipmentKind::Vessel, [-5, 84, 16]),
            (EquipmentKind::Pump, [-3, 84, 16]),
            (EquipmentKind::Valve, [-2, 84, 16]),
            (EquipmentKind::Pipe, [-1, 84, 16]),
            (EquipmentKind::Valve, [2, 86, 16]),
            (EquipmentKind::Pipe, [3, 86, 16]),
            (EquipmentKind::Vessel, [4, 86, 16]),
            (EquipmentKind::Valve, [2, 81, 16]),
            (EquipmentKind::Pipe, [3, 81, 16]),
            (EquipmentKind::Vessel, [4, 81, 16]),
        ] {
            let id = a.build(kind, p).await;
            if p == [-5, 84, 16] {
                feed = id;
            }
            if p == [-1, 84, 16] {
                breakable = id;
            }
        }
        a.teleport([0.5, 81.0, 13.5]).await;
        let report = a.inspect(column.id, 1).await;
        assert_eq!(report.master_kind, EquipmentKind::Column);
        assert_eq!(report.status, AssemblyStatus::Complete, "{report:?}");
        assert_eq!(report.lines.len(), 3);
        assert!(
            b.model.lock().await.assembly_reply.is_none(),
            "reports remain private"
        );
        let other = b.inspect(column.id, 1).await;
        assert_eq!(report, other);
        a.teleport([-4.5, 84.0, 13.5]).await;
        let (yaw, pitch) = aim(a.model.lock().await.body.position, [-4.0, 85.4, 17.0]);
        a.send(ClientMessage::Input3d {
            seq: 1,
            input: PlayerInput {
                yaw,
                pitch,
                ..Default::default()
            },
        })
        .await;
        tokio::time::sleep(Duration::from_millis(550)).await;
        a.chat(&format!("/industry charge {feed} 20 20 330")).await;
        a.wait(|m| !m.terrain.equipment[&feed].contents.is_empty())
            .await;
        let before_facts = manager
            .lock()
            .await
            .terrain
            .equipment
            .iter()
            .map(|(id, d)| (*id, facts(d)))
            .collect::<Vec<_>>();
        let before_ledger = manager.lock().await.industry_ledger().clone();
        tokio::time::sleep(Duration::from_millis(650)).await;
        assert_eq!(
            manager
                .lock()
                .await
                .terrain
                .equipment
                .iter()
                .map(|(id, d)| (*id, facts(d)))
                .collect::<Vec<_>>(),
            before_facts
        );
        assert_eq!(manager.lock().await.industry_ledger(), &before_ledger);
        a.teleport([0.5, 81.0, 13.5]).await;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/column-quic")
            .join(uuid::Uuid::new_v4().to_string())
            .join("world3d.bin");
        let restored = {
            let w = manager.lock().await;
            w.save(&path).unwrap();
            let restored = VoxelWorld::load(&path).unwrap();
            for (id, d) in &w.terrain.equipment {
                assert_eq!(restored.terrain.equipment[id], facts(d));
            }
            assert_eq!(restored.industry_ledger(), w.industry_ledger());
            restored
        };
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../target/column-fixture-path.txt"),
            path.to_string_lossy().replace('\\', "/"),
        )
        .unwrap();
        println!("COLUMN_FIXTURE={}", path.display());
        a.teleport([-0.5, 84.0, 13.5]).await;
        let m = a.model.lock().await;
        let d = &m.terrain.equipment[&breakable];
        let (yaw, pitch) = aim(m.body.position, d.origin.map(|n| n as f32 + 0.5));
        let remove = EquipmentCommand::Remove {
            id: d.id,
            revision: d.revision,
            yaw,
            pitch,
            inventory_revision: m.inventory.revision,
        };
        drop(m);
        a.send(ClientMessage::Equipment(remove)).await;
        a.wait(|m| !m.terrain.equipment.contains_key(&breakable))
            .await;
        b.wait(|m| {
            m.assembly_reply
                .as_ref()
                .is_some_and(|r| r.status == AssemblyStatus::Stale)
        })
        .await;
        a.teleport([0.5, 81.0, 13.5]).await;
        assert_eq!(
            a.inspect(column.id, 2).await.status,
            AssemblyStatus::Incomplete
        );
        drop(a);
        drop(b);
        let _ = stop.send(());
        server.await.unwrap().unwrap();
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
        let reopened = Peer::connect(address, cert, 101).await;
        reopened
            .wait(|m| m.terrain.equipment.contains_key(&column.id))
            .await;
        assert_eq!(
            reopened.model.lock().await.terrain.equipment[&column.id].column_design,
            Some(design)
        );
        assert!(reopened.model.lock().await.assembly_reply.is_none());
        drop(reopened);
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("column construction scenario timed out");
}
