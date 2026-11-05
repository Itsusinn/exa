//! An owned loopback authority. All disk/network work stays off the host thread.
use anyhow::{Context, Result};
use exa_server::{network, voxel::VoxelWorld};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, oneshot, Mutex};

pub enum LocalEvent {
    Ready(SocketAddr, Vec<u8>),
    Finished(Result<()>),
}

pub struct LocalServer {
    events: mpsc::Receiver<LocalEvent>,
    shutdown: Option<oneshot::Sender<()>>,
    pub stopping: bool,
}

impl LocalServer {
    pub fn start(path: PathBuf) -> Result<Self> {
        let (tx, events) = mpsc::channel(2);
        let (shutdown, rx) = oneshot::channel();
        std::thread::Builder::new()
            .name("exa-local-server".into())
            .spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    runtime.block_on(run(path, &tx, rx))
                })();
                let _ = tx.blocking_send(LocalEvent::Finished(result));
            })?;
        Ok(Self {
            events,
            shutdown: Some(shutdown),
            stopping: false,
        })
    }

    pub fn stop(&mut self) {
        self.stopping = true;
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }

    pub fn poll(&mut self) -> Option<LocalEvent> {
        match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => Some(LocalEvent::Finished(Err(
                anyhow::anyhow!("local server worker stopped unexpectedly"),
            ))),
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run(
    path: PathBuf,
    tx: &mpsc::Sender<LocalEvent>,
    shutdown: oneshot::Receiver<()>,
) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    // Held until save completes; a second local client must not overwrite this world.
    let _lock = exa_server::voxel::lock_world(&path)?;
    VoxelWorld::validate_generation()?;
    let mut world = if path.exists() {
        VoxelWorld::load(&path)?
    } else {
        VoxelWorld::new(12345)?
    };
    world.set_save_path(path.clone());
    let manager = Arc::new(Mutex::new(world));
    let (endpoint, certificate) = network::make_server_endpoint("127.0.0.1:0".parse()?)?;
    tx.send(LocalEvent::Ready(endpoint.local_addr()?, certificate))
        .await
        .context("local session closed")?;
    network::serve_endpoint(endpoint, manager.clone(), async {
        let _ = shutdown.await;
    })
    .await?;
    manager
        .lock()
        .await
        .save(&path)
        .context("save single-player world")?;
    Ok(())
}
