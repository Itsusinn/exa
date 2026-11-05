//! Real QUIC lifecycle coverage for the separate bounded design worker.
use exa_client::network_client::{connect_endpoint, read_message, write_message};
use exa_protocol::*;
use exa_server::{
    network::{make_server_endpoint, serve_endpoint},
    voxel::VoxelWorld,
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::{oneshot, Mutex};

#[derive(Default)]
struct Observations {
    ready: bool,
    player_id: u32,
    sequence: u32,
    reports: BTreeMap<u64, DistillationReply>,
    completed: Vec<u64>,
    pong: u32,
}
struct Peer {
    _endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    send: quinn::SendStream,
    seen: Arc<Mutex<Observations>>,
    reader: tokio::task::JoinHandle<()>,
}
impl Peer {
    async fn connect(addr: std::net::SocketAddr, cert: Vec<u8>, identity: u8) -> Self {
        let (endpoint, connection) = connect_endpoint(addr, cert).await.unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        write_message(
            &mut send,
            &ClientMessage::Login {
                version: PROTOCOL_VERSION,
                player_name: format!("designer-{identity}"),
                player_uuid: [identity; 16],
            },
        )
        .await
        .unwrap();
        let seen = Arc::new(Mutex::new(Observations::default()));
        let shared = seen.clone();
        let reader = tokio::spawn(async move {
            while let Ok(message) = read_message(&mut recv).await {
                let mut state = shared.lock().await;
                match message {
                    ServerMessage::Welcome3d { player_id, .. } => {
                        state.ready = true;
                        state.player_id = player_id;
                    }
                    ServerMessage::World3d(snapshot) => {
                        state.sequence = state.sequence.max(snapshot.seq)
                    }
                    ServerMessage::Distillation(reply) => {
                        assert!(reply.valid());
                        if reply.status == DistillationStatus::Completed {
                            state.completed.push(reply.request_id);
                        }
                        state.reports.insert(reply.request_id, reply);
                    }
                    ServerMessage::Pong { seq } => state.pong = seq,
                    _ => (),
                }
            }
        });
        let peer = Self {
            _endpoint: endpoint,
            connection,
            send,
            seen,
            reader,
        };
        peer.wait(|state| state.ready).await;
        peer
    }
    async fn send(&mut self, message: ClientMessage) {
        write_message(&mut self.send, &message).await.unwrap();
    }
    async fn design(&mut self, id: u64, spec: DistillationSpec) {
        self.send(ClientMessage::Distillation(
            DistillationCommand::Calculate {
                request_id: id,
                spec,
            },
        ))
        .await;
    }
    async fn wait(&self, predicate: impl Fn(&Observations) -> bool) {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if predicate(&*self.seen.lock().await) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for design network state");
    }
    async fn report(&self, id: u64, terminal: bool) -> DistillationReply {
        self.wait(|state| {
            state
                .reports
                .get(&id)
                .is_some_and(|reply| !terminal || reply.status != DistillationStatus::Running)
        })
        .await;
        self.seen.lock().await.reports[&id].clone()
    }
    async fn close(self) {
        self.connection.close(0u32.into(), b"test done");
        let _ = self.reader.await;
    }
}

#[tokio::test]
async fn design_session_does_not_keep_authority_removed_player_stream_alive() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let world = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
        let (stop, stopped) = oneshot::channel();
        let authority = tokio::spawn(serve_endpoint(endpoint, world.clone(), async {
            let _ = stopped.await;
        }));
        let mut peer = Peer::connect(address, cert, 74).await;
        peer.design(
            1,
            DistillationSpec {
                trays: 12,
                feed_tray: 6,
                ..Default::default()
            },
        )
        .await;
        let _ = peer.report(1, false).await;
        let player = peer.seen.lock().await.player_id;
        world.lock().await.unregister_player(player);
        tokio::time::timeout(Duration::from_secs(2), peer.connection.closed())
            .await
            .expect("design service held the player reply channel alive after authority removal");
        peer.close().await;
        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(3), authority)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn design_requests_cancel_complete_and_do_not_block_authoritative_inputs() {
    tokio::time::timeout(Duration::from_secs(90), async {
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let world = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
        let (stop, stopped) = oneshot::channel();
        let authority = tokio::spawn(serve_endpoint(endpoint, world.clone(), async {
            let _ = stopped.await;
        }));
        let mut alice = Peer::connect(address, cert.clone(), 71).await;
        let mut bob = Peer::connect(address, cert, 72).await;

        alice
            .design(
                1,
                DistillationSpec {
                    trays: 13,
                    ..Default::default()
                },
            )
            .await;
        let rejected = alice.report(1, true).await;
        assert_eq!(rejected.status, DistillationStatus::Rejected);
        // The same request id replays its original reply, even with changed data.
        alice.design(1, DistillationSpec::default()).await;
        alice.send(ClientMessage::Ping { seq: 1 }).await;
        alice.wait(|state| state.pong == 1).await;
        assert_eq!(alice.report(1, true).await, rejected);

        alice
            .design(
                2,
                DistillationSpec {
                    trays: 12,
                    feed_tray: 6,
                    ..Default::default()
                },
            )
            .await;
        alice
            .send(ClientMessage::Distillation(DistillationCommand::Cancel {
                request_id: 2,
            }))
            .await;
        assert_eq!(
            alice.report(2, true).await.status,
            DistillationStatus::Cancelled
        );
        // A new independent client must not receive another player's private report.
        assert!(bob.seen.lock().await.reports.is_empty());

        let mut request = 3;
        loop {
            alice.design(request, DistillationSpec::default()).await;
            let reply = alice.report(request, false).await;
            if reply.status == DistillationStatus::Running
                || reply.status == DistillationStatus::Completed
            {
                break;
            }
            assert_eq!(reply.status, DistillationStatus::Rejected);
            request += 1;
            assert!(
                request < 20,
                "cancelled worker did not release bounded design slot"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // These requests use the regular authority and transport while design runs.
        alice
            .send(ClientMessage::Input3d {
                seq: 1,
                input: PlayerInput::default(),
            })
            .await;
        bob.send(ClientMessage::Ping { seq: 7 }).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            alice.wait(|state| state.sequence >= 1).await;
            bob.wait(|state| state.pong == 7).await;
        })
        .await
        .expect("design computation blocked input or peer traffic");
        let result = alice.report(request, true).await;
        assert_eq!(
            result.status,
            DistillationStatus::Completed,
            "{}",
            result.message
        );
        let data = result.result.as_ref().unwrap();
        assert_eq!(data.stages.len(), 7);
        assert!(data.distillate_butane_fraction > 0.5 && data.bottoms_butane_fraction < 0.5);
        assert!(data.max_component_residual_mol_s < 1e-6);
        assert!(data.max_energy_residual_w < 0.1);
        assert!(!alice.seen.lock().await.completed.contains(&2));
        assert!(bob.seen.lock().await.reports.is_empty());
        assert!(world.lock().await.terrain.equipment.is_empty());

        // A late cancel for the old request cannot cancel or replace the completed one.
        alice
            .send(ClientMessage::Distillation(DistillationCommand::Cancel {
                request_id: 2,
            }))
            .await;
        alice.send(ClientMessage::Ping { seq: 9 }).await;
        alice.wait(|state| state.pong == 9).await;
        assert_eq!(alice.report(request, true).await, result);
        alice.close().await;
        bob.close().await;
        let _ = stop.send(());
        authority.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn design_disconnect_cancels_work_and_new_session_cannot_receive_old_results() {
    tokio::time::timeout(Duration::from_secs(90), async {
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let world = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
        let (stop, stopped) = oneshot::channel();
        let authority = tokio::spawn(serve_endpoint(endpoint, world.clone(), async {
            let _ = stopped.await;
        }));
        let mut first = Peer::connect(address, cert.clone(), 73).await;
        first
            .design(
                1,
                DistillationSpec {
                    trays: 12,
                    feed_tray: 6,
                    ..Default::default()
                },
            )
            .await;
        let start = first.report(1, false).await;
        assert!(matches!(
            start.status,
            DistillationStatus::Running | DistillationStatus::Completed
        ));
        first.close().await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while world.lock().await.player_count() > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut reopened = Peer::connect(address, cert, 73).await;
        reopened.send(ClientMessage::Ping { seq: 11 }).await;
        reopened.wait(|state| state.pong == 11).await;
        assert!(reopened.seen.lock().await.reports.is_empty());
        let mut request = 1;
        loop {
            reopened.design(request, DistillationSpec::default()).await;
            let reply = reopened.report(request, false).await;
            if reply.status != DistillationStatus::Rejected {
                break;
            }
            request += 1;
            assert!(request < 20);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let answer = reopened.report(request, true).await;
        assert_eq!(
            answer.status,
            DistillationStatus::Completed,
            "{}",
            answer.message
        );
        reopened.close().await;
        let _ = stop.send(());
        authority.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}
