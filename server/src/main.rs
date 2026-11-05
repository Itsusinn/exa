use anyhow::Result;
use exa_server::{config, network, voxel::VoxelWorld};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "exa_server=info".into()),
        )
        .init();
    let save_path =
        PathBuf::from(config::var_os("SAVE_PATH").unwrap_or_else(|| "save/world3d.bin".into()));
    // A failed load is fatal. Never overwrite legacy or malformed saves.
    let _lock = exa_server::voxel::lock_world(&save_path)?;
    VoxelWorld::validate_generation()?;
    let mut world = if save_path.exists() {
        VoxelWorld::load(&save_path)?
    } else {
        VoxelWorld::new(12345)?
    };
    world.set_save_path(save_path.clone());
    let manager = Arc::new(Mutex::new(world));
    let addr = config::var("BIND")
        .unwrap_or_else(|_| "127.0.0.1:9876".into())
        .parse()?;
    let seconds = config::var("RUN_SECONDS")
        .ok()
        .map(|s| s.parse::<u64>())
        .transpose()?;
    // The development launcher owns this pipe. EOF also handles launcher crashes.
    // A plain thread avoids leaving Tokio's blocking pool stuck on stdin at exit.
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let watch_stdin = config::var("SHUTDOWN_ON_STDIN_CLOSE").as_deref() == Ok("1");
    if watch_stdin {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            let _ = stop_tx.send(());
        });
    }
    let shutdown = async {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>{},
            _=stop_rx, if watch_stdin=>{},
            _=async {if let Some(s)=seconds {tokio::time::sleep(std::time::Duration::from_secs(s)).await;} else {std::future::pending::<()>().await;}}=>{},
        }
    };
    network::run_server(addr, manager.clone(), shutdown).await?;
    manager.lock().await.save(&save_path)?;
    tracing::info!(path=%save_path.display(),"3D world saved");
    Ok(())
}
