use exa_client::session::Session;
use exa_protocol::PlayerInput;
use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn test_path(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/singleplayer-tests")
        .join(format!(
            "{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .join("world3d.bin");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    path
}

fn wait(session: &mut Session, done: impl Fn(&Session) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        session.tick();
        if done(session) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "session timed out: {}",
            session.status
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn local_world_connects_plays_saves_unlocks_and_reopens() {
    let path = test_path("lifecycle");
    let mut session = Session::default();
    session.identity_path = path.with_extension("client.uuid");
    session.start_singleplayer(path.clone());
    wait(&mut session, |s| s.model.ready);
    assert!(!session.demo);
    assert!(session.model.actors.iter().any(|a| a.kind == 1));
    let initial = session.model.body.position;
    session.chat("singleplayer session".into());
    for _ in 0..12 {
        session.input(PlayerInput {
            forward: 1.0,
            strafe: 1.0,
            pickup: true,
            ..Default::default()
        });
        std::thread::sleep(Duration::from_millis(34));
        session.tick();
    }
    assert!(session.model.body.position[0] > initial[0]);
    assert!(!session.model.inventory.is_empty());
    assert!(session
        .model
        .log
        .iter()
        .any(|e| e.text.contains("singleplayer session")));
    session.inventory(exa_protocol::InventoryCommand::Move {
        revision: session.model.inventory.revision,
        from: 0,
        to: 35,
        quantity: 1,
    });
    wait(&mut session, |s| s.model.inventory.slots[35].is_some());
    session.inventory(exa_protocol::InventoryCommand::Select { slot: 8 });
    wait(&mut session, |s| s.model.inventory.selected == 8);
    let inventory = session.model.inventory.slots.clone();
    let position = session.model.body.position;
    let mut other = Session::default();
    other.identity_path = path.with_extension("other.uuid");
    other.start_singleplayer(path.clone());
    wait(&mut other, |s| !s.local_running());
    assert!(other.local_error, "same save must be locked");
    session.disconnect();
    wait(&mut session, |s| {
        !s.local_running() && !s.shutdown_pending()
    });
    assert!(!session.local_error, "{}", session.status);
    assert!(session.transport.is_none());
    exa_server::voxel::VoxelWorld::load(&path).unwrap();
    // A new client session reads the same local UUID even after changing name.
    session = Session::default();
    session.identity_path = path.with_extension("client.uuid");
    session.name = "Renamed".into();
    session.start_singleplayer(path);
    wait(&mut session, |s| s.model.ready);
    assert_eq!(session.model.inventory.slots, inventory);
    assert_eq!(session.model.inventory.selected, 8);
    assert!(session.model.items.is_empty());
    assert!(exa_simulation::distance(session.model.body.position, position) < 0.2);
    session.disconnect();
    wait(&mut session, |s| {
        !s.local_running() && !s.shutdown_pending()
    });
}

#[test]
fn corrupt_save_is_reported_and_preserved() {
    let path = test_path("corrupt");
    std::fs::write(&path, b"invalid world").unwrap();
    let mut session = Session::default();
    session.identity_path = path.with_extension("client.uuid");
    session.start_singleplayer(path.clone());
    wait(&mut session, |s| !s.local_running());
    assert!(session.local_error);
    assert!(!session.model.ready);
    assert_eq!(std::fs::read(path).unwrap(), b"invalid world");
}

#[test]
fn save_failure_is_reported_without_replacing_previous_map() {
    let path = test_path("save-error");
    exa_server::voxel::VoxelWorld::new(12345)
        .unwrap()
        .save(&path)
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut session = Session::default();
    session.identity_path = path.with_extension("client.uuid");
    session.start_singleplayer(path.clone());
    wait(&mut session, |s| s.model.ready);
    std::fs::create_dir(path.with_extension("tmp")).unwrap();
    session.disconnect();
    wait(&mut session, |s| {
        !s.local_running() && !s.shutdown_pending()
    });
    assert!(session.local_error);
    assert!(session.status.contains("save single-player world"));
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn cancel_during_start_does_not_connect_a_stale_session() {
    let path = test_path("cancel");
    let mut session = Session::default();
    session.identity_path = path.with_extension("client.uuid");
    session.start_singleplayer(path.clone());
    session.disconnect();
    wait(&mut session, |s| !s.local_running());
    assert!(session.transport.is_none());
    assert!(!session.model.ready);
    assert!(!session.local_error);
    exa_server::voxel::VoxelWorld::load(&path).unwrap();
}
