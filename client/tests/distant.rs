//! Real QUIC validation of render-only terrain, authority edits and view epochs.
//! Server unit tests cover queue backpressure and generation-budget internals.
use exa_client::network_client::{connect_endpoint, read_message, write_message};
use exa_client_core::voxel::Client3d;
use exa_protocol::*;
use exa_server::{
    network::{make_server_endpoint, serve_endpoint},
    voxel::VoxelWorld,
};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, Mutex};

#[derive(Default)]
struct Received {
    model: Client3d,
    epoch: u64,
    center: [i32; 2],
    first_far_near_sections: Option<usize>,
    far_bytes_received: usize,
    max_far_tiles: usize,
    acknowledged: u32,
}

impl Received {
    fn apply(&mut self, message: ServerMessage) {
        let changed_tile = match &message {
            ServerMessage::DistantTile {
                epoch,
                key,
                revision,
                blocks,
            } => Some((*epoch, *key, *revision, blocks.clone())),
            _ => None,
        };
        match &message {
            ServerMessage::Welcome3d { .. } => {
                self.epoch = 0;
                self.center = [0; 2];
            }
            ServerMessage::DistantView { epoch, center } => {
                if *epoch > self.epoch {
                    self.epoch = *epoch;
                    self.center = *center;
                }
            }
            ServerMessage::DistantTile { blocks, .. } => {
                self.first_far_near_sections
                    .get_or_insert(self.model.terrain.blocks.len());
                self.far_bytes_received += blocks.len();
            }
            ServerMessage::World3d(snapshot) => self.acknowledged = snapshot.seq,
            _ => {}
        }
        self.model.apply(message);
        assert_eq!(self.model.distant.epoch, self.epoch);
        let cache = &self.model.distant;
        self.max_far_tiles = self.max_far_tiles.max(cache.tiles.len());
        assert!(cache.tiles.len() <= 1536 && cache.raw_bytes() <= 48 * 1024 * 1024);
        assert!(cache.desired.len() <= 1024);
        if let Some((epoch, key, revision, blocks)) = changed_tile {
            if epoch == self.epoch && cache.desired.contains(&key) {
                let actual = &cache.tiles[&key];
                assert!(actual.revision >= revision);
                if actual.revision == revision {
                    assert_eq!(
                        actual.blocks.as_ref(),
                        blocks.as_slice(),
                        "production cache preserves wire facts"
                    );
                }
            }
        }
        assert!(
            self.model.terrain.blocks.len() <= MAX_LOADED_SECTIONS,
            "render-only terrain must not inflate the collision Grid"
        );
    }

    fn fine_block(&self, point: [i32; 3]) -> Option<(u8, u64)> {
        let key = DistantTileKey {
            origin: [point[0].div_euclid(16) * 16, point[2].div_euclid(16) * 16],
            stride: 1,
        };
        let value = self.model.distant.tiles.get(&key)?;
        let index = point[0].rem_euclid(16) as usize
            + 16 * (point[2].rem_euclid(16) as usize + 16 * point[1] as usize);
        Some((value.blocks[index], value.revision))
    }
}

#[test]
fn shared_hierarchy_partitions_the_horizon_and_preserves_the_fine_guard() {
    fn walk(
        key: DistantTileKey,
        nodes: &BTreeSet<DistantTileKey>,
        leaves: &BTreeSet<DistantTileKey>,
        visited: &mut BTreeSet<DistantTileKey>,
    ) {
        assert!(
            nodes.contains(&key) && visited.insert(key),
            "each planned node has exactly one parent"
        );
        assert_eq!(key.origin[0].rem_euclid(key.span()), 0);
        assert_eq!(key.origin[1].rem_euclid(key.span()), 0);
        if leaves.contains(&key) {
            return;
        }
        assert!(key.stride > 1, "a finest node cannot split");
        let children = key.children().expect("non-leaf node has four children");
        assert_eq!(children.len(), 4);
        let mut area = 0;
        for child in children {
            assert_eq!(child.stride * 2, key.stride);
            assert!(child.origin[0] >= key.origin[0] && child.origin[1] >= key.origin[1]);
            assert!(child.origin[0] + child.span() <= key.origin[0] + key.span());
            assert!(child.origin[1] + child.span() <= key.origin[1] + key.span());
            area += child.span() * child.span();
            walk(child, nodes, leaves, visited);
        }
        assert_eq!(
            area,
            key.span() * key.span(),
            "children cover the complete parent area"
        );
    }
    for center in [[0, 0], [-1, -1], [31, -33], [1023, -1024]] {
        let plan = distant_plan(center);
        let nodes: BTreeSet<_> = plan.nodes.iter().copied().collect();
        assert_eq!(nodes.len(), plan.nodes.len());
        assert!(nodes.len() <= 1024);
        assert_eq!(plan.roots.len(), 81);
        let mut visited = BTreeSet::new();
        for &key in &plan.roots {
            assert_eq!(key.stride, 32);
            walk(key, &nodes, &plan.leaves, &mut visited);
        }
        assert_eq!(visited, nodes, "no orphaned or overlapping hierarchy nodes");
        for dz in -4..=4 {
            for dx in -4..=4 {
                assert!(plan.leaves.contains(&DistantTileKey {
                    origin: [(center[0] + dx) * 16, (center[1] + dz) * 16],
                    stride: 1
                }));
            }
        }
        for dx in [-2048, 2048] {
            for dz in [-2048, 2048] {
                let point = [center[0] * 16 + 8 + dx, center[1] * 16 + 8 + dz];
                assert_eq!(
                    plan.roots
                        .iter()
                        .filter(|key| point[0] >= key.origin[0]
                            && point[0] < key.origin[0] + key.span()
                            && point[1] >= key.origin[1]
                            && point[1] < key.origin[1] + key.span())
                        .count(),
                    1,
                    "2km corner has exactly one root"
                );
            }
        }
    }
}

#[test]
fn superseded_views_and_revisions_cannot_replace_current_guard_facts() {
    let mut model = Client3d::default();
    let key = DistantTileKey {
        origin: [-16, -16],
        stride: 1,
    };
    let tile = |epoch, revision, block| ServerMessage::DistantTile {
        epoch,
        key,
        revision,
        blocks: vec![block; 32768],
    };
    model.apply(ServerMessage::DistantView {
        epoch: 1,
        center: [-1, -1],
    });
    model.apply(tile(1, 7, PLANK));
    model.apply(tile(1, 6, STONE));
    assert_eq!(model.distant.tiles[&key].blocks[0], PLANK);
    model.apply(ServerMessage::DistantView {
        epoch: 2,
        center: [-2, -1],
    });
    model.apply(tile(1, 100, STONE));
    assert_eq!(model.distant.tiles[&key].revision, 7);
    model.apply(tile(2, 8, AIR));
    assert_eq!(model.distant.tiles[&key].blocks[0], AIR);
    model.apply(ServerMessage::DistantView {
        epoch: 1,
        center: [100, 100],
    });
    assert_eq!((model.distant.epoch, model.distant.center), (2, [-2, -1]));
    assert!(
        model.terrain.blocks.is_empty(),
        "render payloads never enter the collision Grid"
    );
}

#[test]
fn multiple_complete_views_keep_render_cache_bounded_without_exploring() {
    let mut model = Client3d::default();
    for (step, center) in [[0, 0], [32, 32], [-33, -33], [96, -96], [-128, 128], [0, 0]]
        .into_iter()
        .enumerate()
    {
        let epoch = step as u64 + 1;
        model.apply(ServerMessage::DistantView { epoch, center });
        for key in distant_plan(center).nodes {
            model.apply(ServerMessage::DistantTile {
                epoch,
                key,
                revision: epoch,
                blocks: vec![STONE; distant_tile_block_count(key).unwrap()],
            });
            assert!(model.distant.tiles.len() <= MAX_CACHED_TILES);
            assert!(model.distant.raw_bytes() <= MAX_DISTANT_CACHE_BYTES);
            assert!(model.distant.desired.len() <= MAX_WANTED_TILES);
        }
        assert!(
            model
                .distant
                .desired
                .iter()
                .all(|key| model.distant.tiles.contains_key(key)),
            "budget must accommodate the complete desired view"
        );
        assert!(model.terrain.blocks.is_empty());
    }
}

struct Peer {
    _endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    send: quinn::SendStream,
    state: Arc<Mutex<Received>>,
    reader: tokio::task::JoinHandle<()>,
}

impl Peer {
    async fn connect(address: std::net::SocketAddr, certificate: Vec<u8>, identity: u8) -> Self {
        let (endpoint, connection) = connect_endpoint(address, certificate).await.unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        write_message(
            &mut send,
            &ClientMessage::Login {
                version: PROTOCOL_VERSION,
                player_name: format!("Distant {identity}"),
                player_uuid: [identity; 16],
            },
        )
        .await
        .unwrap();
        let state = Arc::new(Mutex::new(Received::default()));
        let received = state.clone();
        let reader = tokio::spawn(async move {
            while let Ok(message) = read_message(&mut recv).await {
                received.lock().await.apply(message);
            }
        });
        let peer = Self {
            _endpoint: endpoint,
            connection,
            send,
            state,
            reader,
        };
        peer.wait("login", |s| s.model.ready).await;
        peer
    }

    async fn wait(&self, label: &str, done: impl Fn(&Received) -> bool) {
        self.wait_for(label, Duration::from_secs(60), done).await;
    }

    async fn wait_for(&self, label: &str, duration: Duration, done: impl Fn(&Received) -> bool) {
        let deadline = Instant::now() + duration;
        loop {
            let s = self.state.lock().await;
            if done(&s) {
                return;
            }
            assert!(
                !self.reader.is_finished(),
                "reader failed while waiting for {label}"
            );
            assert!(
                Instant::now() < deadline,
                "{label}: pos={:?} near={} far={} epoch={} center={:?} edit_error={} log={:?}",
                s.model.body.position,
                s.model.terrain.blocks.len(),
                s.model.distant.tiles.len(),
                s.epoch,
                s.center,
                s.model.container_error,
                s.model.log
            );
            drop(s);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn send(&mut self, message: ClientMessage) {
        write_message(&mut self.send, &message).await.unwrap();
    }

    async fn command(&mut self, text: String) {
        // Authority commands have a 15-tick cooldown, including rejected commands.
        tokio::time::sleep(Duration::from_millis(600)).await;
        self.send(ClientMessage::Chat3d { text }).await;
    }

    async fn teleport(&mut self, point: [f32; 3]) {
        self.command(format!("/tp {} {} {}", point[0], point[1], point[2]))
            .await;
        self.wait("teleport acknowledged", |s| {
            exa_simulation::distance(s.model.body.position, point) < 0.01
                && s.center == column_at(point)
        })
        .await;
    }

    async fn close(self) {
        self.connection.close(0u32.into(), b"distant test done");
        self.reader.await.unwrap();
    }
}

#[tokio::test]
async fn distant_geometry_preserves_facts_without_expanding_collision_or_saved_exploration() {
    tokio::time::timeout(Duration::from_secs(240), async {
        let mut world = VoxelWorld::new(12345).unwrap();
        let target = [8,108,8];
        // Construct one complete fixture column before the server starts. Never
        // mutate facts behind the revision/cache system during an active test.
        for y in (0..WORLD_HEIGHT).step_by(16) {
            let origin = [0,y,0];
            if !world.terrain.blocks.contains_key(&origin) {
                assert!(world.terrain.apply_chunk(origin,&[AIR;4096]));
            }
        }
        world.terrain.set(target,PLANK);
        world.terrain.set([8,107,8],STONE);
        let manager = Arc::new(Mutex::new(world));
        let (endpoint, cert) = make_server_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = endpoint.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(serve_endpoint(endpoint,manager.clone(),async { let _ = stopped.await; }));
        let mut observer = Peer::connect(address,cert.clone(),171).await;
        observer.command("/fly on".into()).await;
        observer.wait("flight", |s| s.model.flying).await;
        let home = [8.5,110.0,8.5];
        observer.teleport(home).await;
        observer.wait("near world first", |s| s.model.terrain.blocks.len()==MAX_LOADED_SECTIONS).await;
        let before = manager.lock().await.terrain.columns();
        // Inputs must remain usable while the much larger far cache is filling.
        observer.send(ClientMessage::Input3d { seq:1,input:PlayerInput::default() }).await;
        observer.wait_for("input acknowledgement during distant streaming",Duration::from_secs(3),|s| s.acknowledged>=1).await;
        observer.wait("unknown preview arrives without waiting for the entire 2km horizon", |s| {
            s.model.distant.tiles.keys().any(|key| {
                (0..key.span()).step_by(16).any(|z| (0..key.span()).step_by(16).any(|x| {
                    !before.contains(&[(key.origin[0]+x).div_euclid(16),(key.origin[1]+z).div_euclid(16)])
                }))
            })
        }).await;
        assert_eq!(manager.lock().await.terrain.columns(),before,"preview terrain must remain derived, not explored");
        let s = observer.state.lock().await;
        assert!(s.first_far_near_sections.unwrap()>0,"near chunks must precede far geometry");
        let initial_far_bytes = s.far_bytes_received;
        drop(s);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/distant-quic").join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let save = root.join("world3d.bin");
        manager.lock().await.save(&save).unwrap();
        assert_eq!(VoxelWorld::load(&save).unwrap().terrain.columns(),before,"saving after previews must not persist unknown far columns");

        // Move the observer away from a second player's editable near-world cell.
        // An observer at four columns distance receives the 1m guard without
        // admitting it to the near collision grid.
        let watch = [home[0]-64.0,110.0,home[2]];
        observer.teleport(watch).await;
        observer.wait("landmark in far guard",|s| s.fine_block(target).is_some_and(|v|v.0==PLANK)
            && s.model.terrain.get(target)==255).await;
        let mut editor = Peer::connect(address,cert.clone(),172).await;
        editor.command("/fly on".into()).await;
        editor.wait("editor flight", |s| s.model.flying).await;
        editor.teleport(home).await;
        editor.wait("fixture landmark in near Grid",|s| s.model.terrain.get(target)==PLANK).await;
        let old_revision = observer.state.lock().await.fine_block(target).unwrap().1;
        let inventory_revision = editor.state.lock().await.model.inventory.revision;
        // Shared targeting rejects abs(pitch)>1.5. Verify this legal ray hits
        // both the landmark and its support without changing gameplay limits.
        let pitch = -1.5;
        assert_eq!(exa_simulation::targeting::target_block(&manager.lock().await.terrain,home,0.0,pitch).unwrap().position,target);
        editor.send(ClientMessage::EditBlock(BlockEdit {
            target,expected:PLANK,yaw:0.0,pitch,
            place:false,slot:0,inventory_revision,
        })).await;
        editor.wait("mine reaches near Grid",|s|s.model.terrain.get(target)==AIR).await;
        observer.wait("mine reaches distant-only observer",|s|s.fine_block(target).is_some_and(|v|v.0==AIR&&v.1>old_revision)).await;
        assert_eq!(observer.state.lock().await.model.terrain.get(target),255);
        tokio::time::sleep(Duration::from_millis(250)).await;
        let inventory_revision = editor.state.lock().await.model.inventory.revision;
        let anchor = [target[0],107,target[2]];
        let hit = exa_simulation::targeting::target_block(&manager.lock().await.terrain,home,0.0,pitch).unwrap();
        assert_eq!((hit.position,hit.normal),(anchor,[0,1,0]));
        editor.send(ClientMessage::EditBlock(BlockEdit {
            target:anchor,expected:STONE,yaw:0.0,pitch,
            place:true,slot:0,inventory_revision,
        })).await;
        observer.wait("place reaches distant-only observer",|s|s.fine_block(target).is_some_and(|v|v.0==PLANK&&v.1>old_revision)).await;
        assert_eq!(manager.lock().await.terrain.get(target),PLANK);

        // Change epochs before far work drains, including negative coordinates.
        let epoch_before = observer.state.lock().await.epoch;
        observer.teleport([-256.5,110.0,-256.5]).await;
        observer.teleport(watch).await;
        observer.wait("current epoch restores guard facts after rapid teleport",|s|s.fine_block(target).is_some_and(|v|v.0==PLANK)&&s.model.terrain.blocks.len()==MAX_LOADED_SECTIONS).await;
        assert!(observer.state.lock().await.epoch>epoch_before);
        assert_eq!(observer.state.lock().await.model.distant.center,column_at(watch));
        observer.close().await;
        let observer = Peer::connect(address,cert,171).await;
        observer.wait("reconnected distant facts",|s|s.fine_block(target).is_some_and(|v|v.0==PLANK)).await;
        assert_eq!(observer.state.lock().await.model.terrain.get(target),255);
        manager.lock().await.save(&save).unwrap();
        let restored = VoxelWorld::load(&save).unwrap();
        assert_eq!(restored.terrain.get(target),PLANK);
        let final_far_bytes = observer.state.lock().await.far_bytes_received;
        std::fs::write(root.join("metrics.json"),format!("{{\"passed\":true,\"coverage_scope\":\"bounded guard facts; full 2km coverage belongs to GPU cold review\",\"near_section_limit\":392,\"far_cache_limit\":1536,\"initial_far_bytes_received\":{initial_far_bytes},\"reconnect_far_bytes_received\":{final_far_bytes},\"preview_saved_columns_before\":{},\"target\":[{},{},{}]}}",before.len(),target[0],target[1],target[2])).unwrap();
        println!("DISTANT_QUIC_EVIDENCE={}",root.display());
        observer.close().await;
        editor.close().await;
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    }).await.expect("distant QUIC integration exceeded 240 seconds");
}
