use exa_client::network_client::{
    connect_endpoint, read_message, write_message, NetworkEvent, QuicSession, Transport,
};
use exa_protocol::*;
use exa_server::{
    network::{make_server_endpoint, serve_endpoint},
    voxel::VoxelWorld,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{oneshot, Mutex};

fn generated_chest(world: &VoxelWorld) -> [i32; 3] {
    for (origin, blocks) in &world.terrain.blocks {
        if let Some(i) = blocks.iter().position(|b| *b == CHEST) {
            return [
                origin[0] + (i % 16) as i32,
                origin[1] + (i / 256) as i32,
                origin[2] + ((i / 16) % 16) as i32,
            ];
        }
    }
    panic!("generated world has no chest");
}

#[tokio::test]
async fn stalled_authority_bounds_client_inputs_and_resumes_without_disconnect() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let (_endpoint, connection) = connect_endpoint(address, cert).await.unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        let id = login(&mut send, &mut recv, "Flow control").await;
        let mut model = exa_client_core::voxel::Client3d::default();
        model.apply(ServerMessage::Welcome3d {
            player_id: id,
            seed: 12345,
            rotation: [0.0; 2],
        });
        while !model.ready {
            model.apply(read_message(&mut recv).await.unwrap());
        }
        let mut last_sent = 0;
        for _ in 0..2 {
            // Hold the same lock as generation/checkpointing: no authority inputs
            // can execute while the client keeps sampling at its normal 30 Hz.
            let stalled = manager.lock().await;
            let start_seq = last_sent;
            for _ in 0..40 {
                let mut message = None;
                let result = model.input(PlayerInput::default(), |input| {
                    message = Some(input);
                    Ok(())
                });
                if let Some(message) = message {
                    result.unwrap();
                    let ClientMessage::Input3d { seq, .. } = message else {
                        panic!("expected input");
                    };
                    assert_eq!(seq, last_sent + 1);
                    last_sent = seq;
                    write_message(&mut send, &message).await.unwrap();
                } else {
                    assert!(result.is_err());
                }
                tokio::time::sleep(Duration::from_secs_f32(exa_simulation::DT)).await;
            }
            assert_eq!(last_sent - start_seq, MAX_PENDING_INPUTS as u32);
            assert_eq!(model.pending.len(), MAX_PENDING_INPUTS);
            drop(stalled);
            while !model.pending.is_empty() {
                model.apply(read_message(&mut recv).await.unwrap());
            }
            assert_eq!(manager.lock().await.player_count(), 1);
        }
        // The session is still usable for non-movement messages after recovery.
        write_message(
            &mut send,
            &ClientMessage::Chat3d {
                text: "recovered".into(),
            },
        )
        .await
        .unwrap();
        loop {
            let s = snapshot(&mut recv, last_sent).await;
            if s.log.iter().any(|entry| entry.text.contains("recovered")) {
                break;
            }
        }
        connection.close(0u32.into(), b"done");
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("stalled authority recovery timed out");
}

#[tokio::test]
async fn player_commands_sync_flight_teleport_and_private_feedback_over_quic() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager, async {
            let _ = stopped.await;
        }));
        let (_e1, c1) = connect_endpoint(address, cert.clone()).await.unwrap();
        let (mut send, mut recv) = c1.open_bi().await.unwrap();
        let id = login(&mut send, &mut recv, "Commander").await;
        let initial_spawn = snapshot(&mut recv, 0).await.body.position;
        let (_e2, c2) = connect_endpoint(address, cert).await.unwrap();
        let (mut send2, mut recv2) = c2.open_bi().await.unwrap();
        login(&mut send2, &mut recv2, "Observer").await;
        // Keep the observer receive stream owned and drained throughout the test.
        let observer = tokio::spawn(async move {
            let mut saw_destination = false;
            let mut saw_spawn = false;
            let mut saw_chat = false;
            while let Ok(message) = read_message(&mut recv2).await {
                if let ServerMessage::World3d(s) = message {
                    assert!(!s.log.iter().any(|e| e.text.contains("未知指令")
                        || e.text.contains("飞行已开启")
                        || e.text.contains("已传送至")
                        || e.text.contains("已返回出生点")));
                    saw_spawn |= saw_destination
                        && s.actors.iter().any(|a| {
                            a.id == id && exa_simulation::distance(a.position, initial_spawn) < 0.02
                        });
                    saw_destination |= s
                        .actors
                        .iter()
                        .any(|a| a.id == id && a.position[0] < -200.0);
                    saw_chat |= s.log.iter().any(|e| e.text.contains("public message"));
                    if saw_spawn && saw_chat {
                        return;
                    }
                }
            }
            panic!("observer missed teleport, spawn return or public chat");
        });
        write_message(
            &mut send,
            &ClientMessage::Chat3d {
                text: "/fly on".into(),
            },
        )
        .await
        .unwrap();
        let start = loop {
            let s = snapshot(&mut recv, 0).await;
            if s.flying {
                break s.body.position;
            }
        };
        write_message(
            &mut send,
            &ClientMessage::Input3d {
                seq: 1,
                input: PlayerInput {
                    jump: true,
                    sprint: true,
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
        let airborne = snapshot(&mut recv, 1).await;
        assert!(airborne.body.position[1] > start[1] + 0.3);
        tokio::time::sleep(Duration::from_millis(600)).await;
        write_message(
            &mut send,
            &ClientMessage::Chat3d {
                text: "/tp -200.5 110 300.5".into(),
            },
        )
        .await
        .unwrap();
        let mut model = exa_client_core::voxel::Client3d::default();
        model.apply(ServerMessage::Welcome3d {
            player_id: id,
            seed: 12345,
            rotation: [0.0; 2],
        });
        loop {
            model.apply(read_message(&mut recv).await.unwrap());
            assert!(model.terrain.blocks.len() <= MAX_LOADED_SECTIONS);
            if model.body.position[0] < -200.0 && model.terrain.nearby_known(model.body.position) {
                break;
            }
        }
        assert_eq!(model.body.position, [-200.5, 110.0, 300.5]);
        assert!(model.flying);
        write_message(
            &mut send,
            &ClientMessage::Chat3d {
                text: "/unknown".into(),
            },
        )
        .await
        .unwrap();
        loop {
            let s = snapshot(&mut recv, 1).await;
            if s.log.iter().any(|e| e.text.contains("未知指令")) {
                break;
            }
        }
        write_message(
            &mut send,
            &ClientMessage::Chat3d {
                text: "public message".into(),
            },
        )
        .await
        .unwrap();
        // Ordinary malformed commands must not disconnect the sending player.
        write_message(
            &mut send,
            &ClientMessage::Input3d {
                seq: 2,
                input: PlayerInput {
                    descend: true,
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
        assert!(snapshot(&mut recv, 2).await.body.position[1] < 110.0);
        tokio::time::sleep(Duration::from_millis(600)).await;
        write_message(
            &mut send,
            &ClientMessage::Chat3d {
                text: "/spawn".into(),
            },
        )
        .await
        .unwrap();
        loop {
            model.apply(read_message(&mut recv).await.unwrap());
            assert!(model.terrain.blocks.len() <= MAX_LOADED_SECTIONS);
            if exa_simulation::distance(model.body.position, initial_spawn) < 0.02
                && model.terrain.nearby_known(model.body.position)
            {
                break;
            }
        }
        assert!(model.flying);
        observer.await.unwrap();
        c1.close(0u32.into(), b"done");
        c2.close(0u32.into(), b"done");
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("player commands timed out");
}

#[tokio::test]
async fn exploration_streams_generated_columns_unloads_and_reconnects_at_negative_position() {
    tokio::time::timeout(Duration::from_secs(40), async {
        let mut world = VoxelWorld::new(12345).unwrap();
        let chest = generated_chest(&world);
        // An isolated flat runway removes randomized trees/combat from this movement test.
        for x in (-64..112).step_by(16) {
            for z in (16..48).step_by(16) {
                for y in (0..WORLD_HEIGHT).step_by(16) {
                    let mut blocks = vec![AIR; 4096];
                    if y == 0 {
                        for by in 0..=6 {
                            for bz in 0..16 {
                                for bx in 0..16 {
                                    blocks[bx + 16 * (bz + 16 * by)] = STONE;
                                }
                            }
                        }
                    }
                    world.terrain.apply_chunk([x, y, z], &blocks);
                }
            }
        }
        // Preserve the existing chest block/container pair outside the runway lane.
        world.terrain.set(chest, CHEST);
        let initial_columns = world.terrain.columns().len();
        let manager = Arc::new(Mutex::new(world));
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let server_manager = manager.clone();
        let server = tokio::spawn(serve_endpoint(endpoint, server_manager, async {
            let _ = stopped.await;
        }));
        // Accelerate wall time only; every authority step still uses the shared 1/30 dt.
        let tick_manager = manager.clone();
        let ticker = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(2)).await;
                tick_manager.lock().await.tick();
            }
        });
        let uuid = [91; 16];
        let (_e, c) = connect_endpoint(address, cert.clone()).await.unwrap();
        let (mut send, mut recv) = c.open_bi().await.unwrap();
        let id = login_uuid(&mut send, &mut recv, "Explorer", uuid).await;
        let mut model = exa_client_core::voxel::Client3d::default();
        model.apply(ServerMessage::Welcome3d {
            player_id: id,
            seed: 12345,
            rotation: [0.0, 0.0],
        });
        let mut unloaded = 0;
        let mut seq = 0;
        let mut east = false;
        let final_body = loop {
            let message = read_message(&mut recv).await.unwrap();
            let snapshot = if let ServerMessage::World3d(s) = &message {
                Some(s.clone())
            } else {
                None
            };
            if matches!(message, ServerMessage::UnloadColumn { .. }) {
                unloaded += 1;
            }
            model.apply(message);
            assert!(model.terrain.columns().len() <= 49);
            if let Some(s) = snapshot {
                if !model.ready || s.seq < seq {
                    continue;
                }
                if s.body.position[0] > 80.0 {
                    east = true;
                }
                if east && s.body.position[0] < -35.0 {
                    break s.body;
                }
                seq += 1;
                write_message(
                    &mut send,
                    &ClientMessage::Input3d {
                        seq,
                        input: PlayerInput {
                            strafe: if east { -1.0 } else { 1.0 },
                            ..Default::default()
                        },
                    },
                )
                .await
                .unwrap();
            }
        };
        assert!(unloaded > 0);
        assert!(manager.lock().await.terrain.columns().len() > initial_columns);
        assert!(model.terrain.has_column([-3, 2]));
        assert!(!model.terrain.has_column([5, 2]));
        c.close(0u32.into(), b"reconnect");
        while manager.lock().await.player_count() != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        ticker.abort();
        let _ = ticker.await;
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/streaming-quic");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{}.bin", uuid::Uuid::new_v4()));
        manager.lock().await.save(&path).unwrap();
        *manager.lock().await = VoxelWorld::load(&path).unwrap();
        let (_e2, c2) = connect_endpoint(address, cert).await.unwrap();
        let (mut send2, mut recv2) = c2.open_bi().await.unwrap();
        let id2 = login_uuid(&mut send2, &mut recv2, "Explorer renamed", uuid).await;
        model.apply(ServerMessage::Welcome3d {
            player_id: id2,
            seed: 12345,
            rotation: [0.0, 0.0],
        });
        while !model.ready {
            model.apply(read_message(&mut recv2).await.unwrap());
        }
        assert!((model.body.position[0] - final_body.position[0]).abs() < 0.1);
        assert!(model.terrain.has_column([-3, 2]));
        c2.close(0u32.into(), b"done");
        let _ = stop.send(());
        server.await.unwrap().unwrap();
        println!("STREAMING_TEST_SAVE={}", path.display());
    })
    .await
    .expect("streaming QUIC exploration timed out");
}

async fn snapshot(recv: &mut quinn::RecvStream, seq: u32) -> Snapshot3d {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let ServerMessage::World3d(s) = read_message(recv).await.unwrap() {
                if s.seq >= seq {
                    return s;
                }
            }
        }
    })
    .await
    .expect("3D snapshot timed out")
}
async fn login(send: &mut quinn::SendStream, recv: &mut quinn::RecvStream, name: &str) -> u32 {
    login_uuid(send, recv, name, *uuid::Uuid::new_v4().as_bytes()).await
}
async fn login_uuid(
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    name: &str,
    player_uuid: [u8; 16],
) -> u32 {
    write_message(
        send,
        &ClientMessage::Login {
            version: PROTOCOL_VERSION,
            player_name: name.into(),
            player_uuid,
        },
    )
    .await
    .unwrap();
    match read_message(recv).await.unwrap() {
        ServerMessage::Welcome3d { player_id, .. } => player_id,
        other => panic!("expected 3D welcome, got {other:?}"),
    }
}

#[tokio::test]
async fn block_edits_reach_both_clients_and_consume_the_selected_stack() {
    let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = endpoint.local_addr().unwrap();
    let manager = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
        let _ = stopped.await;
    }));
    let (_e1, c1) = connect_endpoint(address, cert.clone()).await.unwrap();
    let (mut s1, mut r1) = c1.open_bi().await.unwrap();
    login(&mut s1, &mut r1, "Builder").await;
    let initial = snapshot(&mut r1, 0).await;
    let (_e2, c2) = connect_endpoint(address, cert).await.unwrap();
    let (mut s2, mut r2) = c2.open_bi().await.unwrap();
    login(&mut s2, &mut r2, "Observer").await;
    snapshot(&mut r2, 0).await;
    let feet = initial.body.position;
    // Mine beside the player, keeping the authoritative eye stationary.
    let target = [
        feet[0].floor() as i32 + 1,
        feet[1].round() as i32 - 1,
        feet[2].floor() as i32,
    ];
    let initial_offset: [f32; 3] = std::array::from_fn(|i| {
        target[i] as f32 + 0.5
            - feet[i]
            - if i == 1 {
                exa_simulation::EYE_HEIGHT
            } else {
                0.0
            }
    });
    let initial_yaw = initial_offset[0].atan2(-initial_offset[2]);
    let initial_pitch = initial_offset[1].atan2(initial_offset[0].hypot(initial_offset[2]));
    let expected = manager.lock().await.terrain.get(target);
    write_message(
        &mut s1,
        &ClientMessage::EditBlock(BlockEdit {
            target,
            expected,
            yaw: initial_yaw,
            pitch: initial_pitch,
            place: false,
            slot: 0,
            inventory_revision: initial.inventory.revision,
        }),
    )
    .await
    .unwrap();
    async fn changed(recv: &mut quinn::RecvStream, cell: [i32; 3], block: u8) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let message = read_message(recv).await.unwrap();
                if let ServerMessage::World3d(s) = &message {
                    assert!(
                        s.container_error.is_empty(),
                        "edit rejected for {cell:?}: {}",
                        s.container_error
                    );
                }
                if let ServerMessage::Chunk3d { origin, blocks } = message {
                    let mut map = exa_simulation::grid::Grid::unknown();
                    if map.apply_chunk(origin, &blocks) && map.get(cell) == block {
                        break;
                    }
                }
            }
        })
        .await
        .expect("terrain edit did not reach peer");
    }
    changed(&mut r1, target, AIR).await;
    changed(&mut r2, target, AIR).await;
    let mined = snapshot(&mut r1, 0).await;
    assert_eq!(
        mined.inventory.slots[0].as_ref().unwrap().name,
        block_item_name(expected).unwrap()
    );
    assert_eq!(mined.inventory.slots[0].as_ref().unwrap().quantity, 1);
    tokio::time::sleep(Duration::from_millis(240)).await;
    let current = snapshot(&mut r1, 0).await;
    let anchor = [target[0], target[1] - 1, target[2]];
    let eye = [
        current.body.position[0],
        current.body.position[1] + exa_simulation::EYE_HEIGHT,
        current.body.position[2],
    ];
    let offset: [f32; 3] = std::array::from_fn(|i| anchor[i] as f32 + 0.5 - eye[i]);
    let yaw = offset[0].atan2(-offset[2]);
    let pitch = offset[1].atan2(offset[0].hypot(offset[2]));
    let (hit, expected_anchor) = {
        let w = manager.lock().await;
        (
            exa_simulation::targeting::target_block(&w.terrain, current.body.position, yaw, pitch)
                .unwrap(),
            w.terrain.get(anchor),
        )
    };
    assert_eq!(hit.position, anchor);
    let placed = std::array::from_fn(|i| hit.position[i] + hit.normal[i]);
    write_message(
        &mut s1,
        &ClientMessage::EditBlock(BlockEdit {
            target: anchor,
            expected: expected_anchor,
            yaw,
            pitch,
            place: true,
            slot: 0,
            inventory_revision: current.inventory.revision,
        }),
    )
    .await
    .unwrap();
    changed(&mut r1, placed, expected).await;
    changed(&mut r2, placed, expected).await;
    assert!(snapshot(&mut r1, 0).await.inventory.is_empty());
    assert_eq!(manager.lock().await.terrain.get(placed), expected);
    c1.close(0u32.into(), b"done");
    c2.close(0u32.into(), b"done");
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn stable_uuid_reconnects_after_rename_and_rejects_duplicate_online_identity() {
    let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = endpoint.local_addr().unwrap();
    let manager = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
        let _ = stopped.await;
    }));
    let (_e1, c1) = connect_endpoint(address, cert.clone()).await.unwrap();
    let (mut s1, mut r1) = c1.open_bi().await.unwrap();
    let first = login_uuid(&mut s1, &mut r1, "Original", [5; 16]).await;
    snapshot(&mut r1, 0).await;
    write_message(
        &mut s1,
        &ClientMessage::Inventory(InventoryCommand::Select { slot: 7 }),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while snapshot(&mut r1, 0).await.inventory.selected != 7 {}
    })
    .await
    .unwrap();
    let (_e2, c2) = connect_endpoint(address, cert.clone()).await.unwrap();
    let (mut s2, mut r2) = c2.open_bi().await.unwrap();
    write_message(
        &mut s2,
        &ClientMessage::Login {
            version: PROTOCOL_VERSION,
            player_name: "Duplicate".into(),
            player_uuid: [5; 16],
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), read_message(&mut r2))
            .await
            .unwrap()
            .unwrap(),
        ServerMessage::Error { code: 409, .. }
    ));
    assert_eq!(manager.lock().await.player_count(), 1);
    c1.close(0u32.into(), b"reconnect");
    tokio::time::timeout(Duration::from_secs(3), async {
        while manager.lock().await.player_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (_e3, c3) = connect_endpoint(address, cert.clone()).await.unwrap();
    let (mut s3, mut r3) = c3.open_bi().await.unwrap();
    let second = login_uuid(&mut s3, &mut r3, "Renamed", [5; 16]).await;
    assert_ne!(
        first, second,
        "connection IDs remain separate from persistent UUID"
    );
    assert_eq!(snapshot(&mut r3, 0).await.inventory.selected, 7);
    let (_e4, c4) = connect_endpoint(address, cert).await.unwrap();
    let (mut s4, mut r4) = c4.open_bi().await.unwrap();
    login_uuid(&mut s4, &mut r4, "Renamed", [6; 16]).await;
    assert_eq!(
        snapshot(&mut r4, 0).await.inventory.selected,
        0,
        "same nickname with different UUID is a different player"
    );
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn shared_chest_transfers_over_quic_reject_stale_requests_and_survive_reopen() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
        let position = generated_chest(&*manager.lock().await);
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let (_e1, c1) = connect_endpoint(address, cert.clone()).await.unwrap();
        let (mut s1, mut r1) = c1.open_bi().await.unwrap();
        login(&mut s1, &mut r1, "ChestAlice").await;
        snapshot(&mut r1, 0).await;
        // Supplies and the chest now follow natural safe ground.
        for seq in 1..=30 {
            write_message(
                &mut s1,
                &ClientMessage::Input3d {
                    seq,
                    input: PlayerInput {
                        pickup: true,
                        ..Default::default()
                    },
                },
            )
            .await
            .unwrap();
            snapshot(&mut r1, seq).await;
        }
        let alice = snapshot(&mut r1, 30).await;
        assert_eq!(alice.inventory.len(), 1);
        write_message(
            &mut s1,
            &ClientMessage::Inventory(InventoryCommand::Move {
                revision: 1,
                from: 0,
                to: 9,
                quantity: 1,
            }),
        )
        .await
        .unwrap();
        loop {
            let s = snapshot(&mut r1, 30).await;
            if s.inventory.revision == 2 {
                assert!(s.inventory.slots[0].is_none());
                assert!(s.inventory.slots[9].is_some());
                break;
            }
        }
        write_message(
            &mut s1,
            &ClientMessage::Inventory(InventoryCommand::Move {
                revision: 1,
                from: 9,
                to: 2,
                quantity: 1,
            }),
        )
        .await
        .unwrap();
        loop {
            let s = snapshot(&mut r1, 30).await;
            if !s.container_error.is_empty() {
                assert!(s.inventory.slots[9].is_some());
                assert!(s.inventory.slots[2].is_none());
                break;
            }
        }
        write_message(
            &mut s1,
            &ClientMessage::Inventory(InventoryCommand::QuickMove {
                revision: 2,
                slot: 9,
            }),
        )
        .await
        .unwrap();
        write_message(
            &mut s1,
            &ClientMessage::Inventory(InventoryCommand::Select { slot: 8 }),
        )
        .await
        .unwrap();
        loop {
            let s = snapshot(&mut r1, 30).await;
            if s.inventory.revision == 3 && s.inventory.selected == 8 {
                assert!(s.inventory.slots[9].is_none());
                assert!(s.inventory.slots[0].is_some());
                break;
            }
        }
        let (_e2, c2) = connect_endpoint(address, cert).await.unwrap();
        let (mut s2, mut r2) = c2.open_bi().await.unwrap();
        login(&mut s2, &mut r2, "ChestBob").await;
        snapshot(&mut r2, 0).await;
        // Joiners can spawn on a different river bank; choose a supported approach
        // with line of sight instead of assuming the old flat-room coordinates.
        let approach = {
            let w = manager.lock().await;
            let mut candidates = Vec::new();
            for y in position[1] - 1..=position[1] + 2 {
                for z in position[2] - 2..=position[2] + 2 {
                    for x in position[0] - 2..=position[0] + 2 {
                        let p = [x as f32 + 0.5, y as f32, z as f32 + 0.5];
                        if w.terrain.solid([x, y - 1, z])
                            && w.terrain.get([x, y - 1, z]) != WATER
                            && !exa_simulation::overlaps(p, &|v| w.terrain.solid(v))
                            && exa_simulation::distance(p, alice.body.position) > 1.5
                            && exa_simulation::container_reachable(&w.terrain, p, position)
                        {
                            candidates.push(p);
                        }
                    }
                }
            }
            *candidates
                .first()
                .expect("generated chest has a second safe approach")
        };
        write_message(
            &mut s2,
            &ClientMessage::Chat3d {
                text: format!("/tp {} {} {}", approach[0], approach[1], approach[2]),
            },
        )
        .await
        .unwrap();
        loop {
            let state = snapshot(&mut r2, 0).await;
            if exa_simulation::distance(state.body.position, approach) < 0.1 {
                break;
            }
        }
        for send in [&mut s1, &mut s2] {
            write_message(
                send,
                &ClientMessage::Container(ContainerCommand::Open { position }),
            )
            .await
            .unwrap();
        }
        for recv in [&mut r1, &mut r2] {
            loop {
                let state = snapshot(recv, 0).await;
                assert!(
                    state.container_error.is_empty(),
                    "chest open failed at {:?}: {}",
                    state.body.position,
                    state.container_error
                );
                if state.container.is_some() {
                    break;
                }
            }
        }
        let command = |revision, deposit| {
            ClientMessage::Container(ContainerCommand::Transfer {
                position,
                revision,
                inventory_revision: if deposit && revision == 0 {
                    3
                } else {
                    u64::from(deposit)
                },
                slot: 0,
                target: None,
                quantity: 1,
                deposit,
            })
        };
        write_message(&mut s1, &command(0, true)).await.unwrap();
        loop {
            let s = snapshot(&mut r2, 0).await;
            if s.container.as_ref().is_some_and(|c| c.revision == 1) {
                assert_eq!(s.container.unwrap().items[0].quantity, 1);
                break;
            }
        }
        // Two clients observed revision 1. Only the first withdrawal may commit.
        write_message(&mut s2, &command(1, false)).await.unwrap();
        loop {
            let s = snapshot(&mut r2, 0).await;
            if !s.inventory.is_empty() {
                assert_eq!(s.inventory.slots[0].as_ref().unwrap().quantity, 1);
                break;
            }
        }
        write_message(&mut s1, &command(1, false)).await.unwrap();
        loop {
            let s = snapshot(&mut r1, 0).await;
            if !s.container_error.is_empty() {
                assert!(s.inventory.is_empty());
                break;
            }
        }
        write_message(&mut s2, &command(2, true)).await.unwrap();
        loop {
            if snapshot(&mut r2, 0)
                .await
                .container
                .is_some_and(|c| c.revision == 3)
            {
                break;
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/chest-quic");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("world-{}.bin", std::process::id()));
        manager.lock().await.save(&path).unwrap();
        c1.close(0u32.into(), b"test complete");
        c2.close(0u32.into(), b"test complete");
        let _ = stop.send(());
        server.await.unwrap().unwrap();
        let mut restored = VoxelWorld::load(&path).unwrap();
        assert_eq!(restored.terrain.get(position), CHEST);
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        restored.register_player(42, tx).unwrap();
        restored.container_command(42, ContainerCommand::Open { position });
        restored.tick();
        let mut loaded_contents = None;
        while let Ok(message) = rx.try_recv() {
            if let ServerMessage::World3d(s) = message {
                loaded_contents = s.container;
            }
        }
        let loaded = loaded_contents.expect("saved chest is usable after restart");
        assert_eq!(
            loaded.items,
            alice.inventory.iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(loaded.revision, 3);
    })
    .await
    .expect("chest QUIC integration timed out");
}
#[tokio::test]
async fn two_clients_receive_voxels_move_jump_chat_and_disconnect_over_verified_quic() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let manager = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
            let _ = stopped.await;
        }));
        let (_e1, c1) = connect_endpoint(address, cert.clone()).await.unwrap();
        let (mut s1, mut r1) = c1.open_bi().await.unwrap();
        let id1 = login(&mut s1, &mut r1, "Alice").await;
        let mut map = exa_simulation::grid::Grid::unknown();
        let mut chunks = 0;
        let initial = loop {
            match read_message(&mut r1).await.unwrap() {
                ServerMessage::Chunk3d { origin, blocks } => {
                    assert!(map.apply_chunk(origin, &blocks));
                    chunks += 1;
                }
                ServerMessage::World3d(s) if map.nearby_known(s.body.position) => break s,
                _ => {}
            }
        };
        assert!((72..=MAX_LOADED_SECTIONS).contains(&chunks));
        assert!(map.nearby_known(initial.body.position));
        let mut support = initial.body.position.map(|v| v.floor() as i32);
        support[1] -= 1;
        assert!(map.solid(support));
        let (_e2, c2) = connect_endpoint(address, cert).await.unwrap();
        let (mut s2, mut r2) = c2.open_bi().await.unwrap();
        let id2 = login(&mut s2, &mut r2, "Bob").await;
        let bob = snapshot(&mut r2, 0).await;
        assert!(bob.actors.iter().any(|a| a.id == id1));
        assert_ne!(id1, id2);
        write_message(
            &mut s1,
            &ClientMessage::Input3d {
                seq: 1,
                input: PlayerInput {
                    forward: 1.0,
                    jump: true,
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
        let moved = snapshot(&mut r1, 1).await;
        assert!(moved.body.position[2] < initial.body.position[2]);
        assert!(moved.body.position[1] > initial.body.position[1] + 0.1);
        assert!(moved.actors.iter().any(|a| a.id == id2));
        let observed = loop {
            let s = snapshot(&mut r2, 0).await;
            if s.actors
                .iter()
                .any(|a| a.id == id1 && a.position[1] > initial.body.position[1] + 0.1)
            {
                break s;
            }
        };
        assert_eq!(
            observed.seq, 0,
            "ack is per player, never the simulation tick"
        );
        write_message(
            &mut s1,
            &ClientMessage::Chat3d {
                text: "hello in 3D".into(),
            },
        )
        .await
        .unwrap();
        loop {
            let s = snapshot(&mut r2, 0).await;
            if s.log.iter().any(|e| e.text.contains("hello in 3D")) {
                break;
            }
        }
        write_message(&mut s2, &ClientMessage::Logout)
            .await
            .unwrap();
        loop {
            let s = snapshot(&mut r1, 1).await;
            if !s.actors.iter().any(|a| a.id == id2) {
                break;
            }
        }
        write_message(&mut s1, &ClientMessage::Logout)
            .await
            .unwrap();
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    })
    .await
    .expect("3D integration test timed out");
}
#[tokio::test]
async fn quic_transport_delivers_3d_snapshots_and_closes_remote_session() {
    let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = endpoint.local_addr().unwrap();
    let manager = Arc::new(Mutex::new(VoxelWorld::new(12345).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
        let _ = stopped.await;
    }));
    let identity = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/network-identities")
        .join(format!("{}.uuid", uuid::Uuid::new_v4()));
    let mut session =
        QuicSession::connect_identity(address, "adapter".into(), cert, identity).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match session.poll() {
                Some(NetworkEvent::Message(ServerMessage::World3d(_))) => break,
                Some(NetworkEvent::Disconnected(reason)) => panic!("{reason}"),
                _ => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await
    .unwrap();
    drop(session);
    tokio::time::timeout(Duration::from_secs(3), async {
        while manager.lock().await.player_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("remote session leaked");
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}
#[tokio::test]
async fn old_protocol_cannot_join_the_3d_world() {
    let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = endpoint.local_addr().unwrap();
    let manager = Arc::new(Mutex::new(VoxelWorld::new(1).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve_endpoint(endpoint, manager, async {
        let _ = stopped.await;
    }));
    let (_endpoint, connection) = connect_endpoint(address, cert).await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    write_message(
        &mut send,
        &ClientMessage::Login {
            version: PROTOCOL_VERSION - 1,
            player_name: "old".into(),
            player_uuid: [1; 16],
        },
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), read_message(&mut recv))
            .await
            .unwrap()
            .is_err()
    );
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn wrong_certificate_is_rejected_and_missing_file_surfaces_asynchronously() {
    let (endpoint, _) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = endpoint.local_addr().unwrap();
    let (_other, wrong_cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let manager = Arc::new(Mutex::new(VoxelWorld::new(1).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve_endpoint(endpoint, manager.clone(), async {
        let _ = stopped.await;
    }));
    assert!(connect_endpoint(address, wrong_cert).await.is_err());
    assert_eq!(manager.lock().await.player_count(), 0);
    let mut transport = QuicSession::connect_file_identity(
        address,
        "missing".into(),
        format!("target/nonexistent-certificate-{}.der", std::process::id()),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/network-identities")
            .join(format!("{}.uuid", uuid::Uuid::new_v4())),
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(NetworkEvent::Disconnected(reason)) = transport.poll() {
                assert!(reason.contains("read server certificate"));
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(transport);
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}
