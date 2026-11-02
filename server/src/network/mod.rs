use anyhow::{Context, Result};
use quinn::{Connection, Endpoint, RecvStream, SendStream, ServerConfig};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, instrument, span, warn, Level};
use tracing_futures::Instrument;

use protocol::{ClientMessage, ServerMessage};

use crate::voxel::VoxelWorld;
mod distillation;
use distillation::DesignService;
type IndustryJob =
    tokio::task::JoinHandle<(u64, std::result::Result<crate::process::FlowStep, String>)>;

#[instrument(level = "info")]
pub fn make_server_endpoint(addr: std::net::SocketAddr) -> Result<(Endpoint, Vec<u8>)> {
    let certified_key = generate_simple_self_signed(vec!["localhost".to_string()])
        .context("generate self-signed cert")?;

    let cert_der = certified_key.cert.der().to_vec();
    let key_der = certified_key.key_pair.serialize_der();

    let tls_config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(cert_der.clone())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
    )
    .context("build rustls server config")?;

    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
        .context("convert to quic server config")?;

    let server_config = ServerConfig::with_crypto(Arc::new(quic_config));
    let endpoint = Endpoint::server(server_config, addr).context("create quinn endpoint")?;

    info!("TLS endpoint created with self-signed cert");
    Ok((endpoint, cert_der))
}

/// Run the QUIC accept loop until either the endpoint closes or `shutdown` resolves.
#[instrument(level = "info", skip(wm, shutdown))]
pub async fn run_server(
    addr: std::net::SocketAddr,
    wm: Arc<Mutex<VoxelWorld>>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let (endpoint, certificate) = make_server_endpoint(addr)?;
    let cert_path = std::path::PathBuf::from(
        std::env::var("CDDA_CERT_PATH").unwrap_or_else(|_| "save/server.cert.der".into()),
    );
    if let Some(parent) = cert_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cert_path, certificate)?;
    serve_endpoint(endpoint, wm, shutdown).await
}

pub async fn serve_endpoint(
    endpoint: Endpoint,
    wm: Arc<Mutex<VoxelWorld>>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    info!("Server listening");

    tokio::pin!(shutdown);
    let mut tick = tokio::time::interval(std::time::Duration::from_secs_f32(game_simulation::DT));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut connections = tokio::task::JoinSet::new();
    let mut checkpoint = tokio::time::interval(std::time::Duration::from_secs(30));
    checkpoint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    checkpoint.tick().await;
    let mut industry_clock = tokio::time::interval(std::time::Duration::from_millis(200));
    industry_clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut industry_job: Option<IndustryJob> = None;
    let designer = Arc::new(Mutex::new(DesignService::default()));

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("Shutdown signal received, stopping accept loop");
                break;
            }
            _ = tick.tick() => {
                wm.lock().await.tick();
                designer.lock().await.poll().await;
            }
            _ = industry_clock.tick() => {
                // One bounded immutable batch; a slow solve cannot enqueue catch-up work or block player ticks.
                if industry_job.as_ref().is_some_and(|job|job.is_finished()) {
                    match industry_job.take().unwrap().await {
                        Ok((revision,result))=>{ wm.lock().await.apply_industry_result(revision,result); }
                        Err(e)=>{ error!(error=%e,"industrial worker failed; previous facts preserved"); }
                    }
                }
                if industry_job.is_none() {
                    if let Some((revision,devices))=wm.lock().await.industry_input() {
                        industry_job=Some(tokio::task::spawn_blocking(move || {
                            let result=std::panic::catch_unwind(||crate::process::solve(&devices,0.2))
                                .unwrap_or_else(|_|Err("industrial worker panicked; previous inventory preserved".into()));
                            (revision,result)
                        }));
                    }
                }
            }
            _ = checkpoint.tick() => {
                if let Err(e) = wm.lock().await.checkpoint() { error!(error=%e, "world checkpoint failed; previous save preserved"); }
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
            incoming = endpoint.accept() => {
                match incoming {
                    Some(incoming) => {
                        let wm = wm.clone();
                        let designer = designer.clone();
                        let peer = incoming.remote_address();
                        connections.spawn(
                            async move {
                                match incoming.await {
                                    Ok(connection) => {
                                        let addr = connection.remote_address();
                                        info!(%addr, "New QUIC connection");
                                        if let Err(e) = handle_connection(connection, wm, designer).await {
                                            error!(%addr, error = %e, "Connection error");
                                        }
                                    }
                                    Err(e) => {
                                        error!(%peer, error = %e, "Incoming connection accept failed");
                                    }
                                }
                            }
                            .instrument(span!(Level::INFO, "accept_conn", %peer)),
                        );
                    }
                    None => {
                        warn!("Endpoint closed, stopping server");
                        break;
                    }
                }
            }
        }
    }

    endpoint.close(0u32.into(), b"shutdown");
    // Finish message handlers before the caller takes its final save snapshot.
    while connections.join_next().await.is_some() {}
    designer.lock().await.shutdown().await;
    // The bounded worker cannot outlive its server session. Discard its result on shutdown;
    // callers persist the last atomically committed batch, never a partial transfer.
    if let Some(job) = industry_job {
        let _ = job.await;
    }
    Ok(())
}

#[instrument(level = "debug", skip(connection, wm, designer), fields(addr = %connection.remote_address()))]
async fn handle_connection(
    connection: Connection,
    wm: Arc<Mutex<VoxelWorld>>,
    designer: Arc<Mutex<DesignService>>,
) -> Result<()> {
    let (send, recv) =
        tokio::time::timeout(std::time::Duration::from_secs(10), connection.accept_bi()).await??;
    let (mut send, mut recv) = (send, recv);

    // Read login message
    let frame =
        tokio::time::timeout(std::time::Duration::from_secs(10), read_frame(&mut recv)).await??;
    let msg: ClientMessage = rmp_serde::from_slice(&frame).context("deserialize login")?;

    let (pid, player_rx, design_tx) = match msg {
        ClientMessage::Login {
            version,
            player_name,
            player_uuid,
        } => {
            anyhow::ensure!(
                version == protocol::PROTOCOL_VERSION,
                "protocol version mismatch"
            );
            anyhow::ensure!(player_name.len() <= 128, "player name too long");
            let mut manager = wm.lock().await;
            let pid = manager.allocate_player_id();
            let (player_tx, player_rx) = mpsc::channel::<ServerMessage>(64);

            let world_seed = manager.world_seed;
            let rotation = match manager.register_profile(
                pid,
                player_uuid,
                player_name.clone(),
                player_tx.clone(),
            ) {
                Ok(rotation) => rotation,
                Err(e) => {
                    drop(manager);
                    write_frame(
                        &mut send,
                        &rmp_serde::to_vec(&ServerMessage::Error {
                            code: 409,
                            text: e.to_string(),
                        })?,
                    )
                    .await?;
                    let _ = send.finish();
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), send.stopped())
                        .await;
                    return Err(e);
                }
            };
            drop(manager);
            if let Err(e) = write_frame(
                &mut send,
                &rmp_serde::to_vec(&ServerMessage::Welcome3d {
                    player_id: pid,

                    seed: world_seed,
                    rotation,
                })?,
            )
            .await
            {
                wm.lock().await.unregister_player(pid);
                return Err(e);
            }
            info!(player_id = pid, %player_name, "Player logged in");
            (pid, player_rx, player_tx)
        }
        _ => anyhow::bail!("expected login message"),
    };

    designer
        .lock()
        .await
        .register(pid, design_tx, connection.clone());

    // Spawn forwarder: SubWorld → client
    let send_arc = Arc::new(Mutex::new(send));
    let send_clone = send_arc.clone();
    let writer_connection = connection.clone();
    let forwarder_span = span!(Level::DEBUG, "msg_forwarder", player_id = pid);
    let forwarder = tokio::spawn(
        async move {
            let mut rx = player_rx;
            while let Some(msg) = rx.recv().await {
                let data = match rmp_serde::to_vec(&msg) {
                    Ok(d) => d,
                    Err(e) => {
                        error!(error = %e, "Serialize server message");
                        break;
                    }
                };
                let mut s = send_clone.lock().await;
                if write_frame(&mut s, &data).await.is_err() {
                    break;
                }
            }
            debug!("Message forwarder stopped");
            writer_connection.close(0u32.into(), b"server stream ended");
        }
        .instrument(forwarder_span),
    );

    let player_id = pid;

    // Main loop: handle client messages
    let result: Result<()> = async {
        loop {
            let frame = match read_frame(&mut recv).await {
                Ok(f) => f,
                Err(_) => break,
            };

            let msg: ClientMessage = match rmp_serde::from_slice(&frame) {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, "Deserialize client message failed");
                    continue;
                }
            };

            match msg {
                ClientMessage::Input3d { seq, input } => {
                    wm.lock().await.input(player_id, seq, input)?;
                }
                ClientMessage::Chat3d { text } => {
                    wm.lock().await.chat(player_id, text)?;
                }
                ClientMessage::Container(command) => {
                    wm.lock().await.container_command(player_id, command);
                }
                ClientMessage::Inventory(command) => {
                    wm.lock().await.inventory_command(player_id, command);
                }
                ClientMessage::EditBlock(command) => {
                    wm.lock().await.edit_block(player_id, command);
                }
                ClientMessage::Equipment(command) => {
                    wm.lock().await.equipment_command(player_id, command);
                }
                ClientMessage::Distillation(command) => {
                    designer.lock().await.command(player_id, command);
                }
                ClientMessage::Ping { seq } => {
                    let pong = ServerMessage::Pong { seq };
                    let data = rmp_serde::to_vec(&pong)?;
                    let mut send = send_arc.lock().await;
                    write_frame(&mut send, &data).await?;
                }
                ClientMessage::Logout => {
                    info!("Player logged out");
                    break;
                }
                _ => {
                    debug!("Unknown message type from client");
                }
            }
        }
        Ok(())
    }
    .await;

    // Cleanup
    designer.lock().await.unregister(player_id);
    let mut wm = wm.lock().await;
    wm.unregister_player(player_id);
    if let Err(e) = wm.checkpoint() {
        error!(error=%e, "disconnect checkpoint failed; player retained in memory");
    }
    forwarder.abort();
    info!(player_id, "Player disconnected");
    connection.close(0u32.into(), b"bye");
    result
}

#[instrument(level = "trace")]
async fn read_frame(recv: &mut RecvStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .context("read frame length")?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 1_048_576 {
        anyhow::bail!("frame too large: {len}");
    }

    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await.context("read frame body")?;
    Ok(buf)
}

#[instrument(level = "trace", skip(data))]
async fn write_frame(send: &mut SendStream, data: &[u8]) -> Result<()> {
    let len = data.len() as u32;
    send.write_all(&len.to_be_bytes())
        .await
        .context("write frame length")?;
    send.write_all(data).await.context("write frame body")?;
    Ok(())
}
