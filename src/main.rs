//! This is a simple imitation of the basic functionality of kubectl:
//! kubectl {get, delete, apply, watch, edit} <resource> [name]
//! with labels and namespace selectors supported.
use std::{net::SocketAddr, sync::Arc};

use futures::TryFutureExt;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing_subscriber::{prelude::*, EnvFilter};

use crate::kube_cache::{Cache, KubeServer, ServerDeploymentStatus};
use crate::opaque_error::OpaqueError;
use crate::packets::clientbound::status::StatusStructNew;
use crate::packets::serverbound::handshake::Handshake;
use crate::packets::{Packet, SendPacket};

mod kube_cache;
mod mc_server;
mod opaque_error;
mod packets;
mod types;

#[tokio::main]
async fn main() {
    // ---- Tracing setup ----
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_level(true);
    let filter_layer = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")); // default to INFO
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(filter_layer)
        .with(tracing_error::ErrorLayer::default())
        .init();

    let commit_hash: &'static str = env!("COMMIT_HASH");
    tracing::info!("revision: {}", commit_hash);

    let cache = kube_cache::Cache::create().unwrap();
    let arc_cache = Arc::new(Mutex::new(cache));
    tracing::info!("initialized kube api");

    let listener = TcpListener::bind("0.0.0.0:25565").await.unwrap();
    tracing::info!("started tcp server");

    loop {
        let (socket, addr) = listener.accept().await.unwrap();
        let acc = arc_cache.clone();

        tokio::spawn(async move {
            tracing::debug!(
                addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
                "Client connected"
            );
            if let Err(e) = process_connection(socket, addr, acc).await {
                tracing::error!(
                    message = format!("Client disconnected"),
                    // addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
                    trace = format!("{}", e.get_span_trace()),
                    err = format!("{}", e.context)
                );
            } else {
                tracing::debug!(
                    addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
                    "Client disconnected"
                );
            }
        });
    }
}

#[tracing::instrument(level = "info", skip(cache, client_stream))]
async fn process_connection(
    mut client_stream: TcpStream,
    addr: SocketAddr,
    cache: Arc<Mutex<kube_cache::Cache>>,
) -> Result<(), OpaqueError> {
    let client_packet = match Packet::parse(&mut client_stream).await {
        Some(x) => x,
        None => {
            // This is debug, because Packet::parse has all error cases logged with tracing::error
            tracing::debug!("Client HANDSHAKE -> malformed packet; Disconnecting...");
            return Ok(());
        }
    };

    // --- Handshake ---
    let handshake;
    let next_server_state;
    if client_packet.id.get_int() != 0 {
        return Err(OpaqueError::create(
            "Client HANDSHAKE -> bad packet; Disconnecting...",
        ));
    }
    handshake = packets::serverbound::handshake::Handshake::parse(client_packet)
        .await
        .ok_or_else(|| "handshake request from client failed to parse".to_string())?;

    next_server_state = handshake.get_next_state();

    let kube_server = KubeServer::create(cache.clone(), &handshake.get_server_address()).await?;
    tracing::debug!(
        "kube server status: {:?}",
        kube_server.get_server_status().await?
    );

    match next_server_state {
        packets::ProtocolState::Status => {
            handle_status(&mut client_stream, &handshake, kube_server).await?;
        }
        packets::ProtocolState::Login => {
            handle_login(&mut client_stream, &handshake, kube_server, cache.clone()).await?
        }
        packets::ProtocolState::Transfer => {
            return Err(OpaqueError::create(
                "next state is transfer; Not yet implemented!",
            ))
        }
        // This is used becuase Handshake::parse returns none if is something else
        _ => unreachable!(),
    };
    Ok(())
}

#[tracing::instrument(level = "info", fields(server_addr = kube_server.get_server_addr()),skip(client_stream, handshake, kube_server))]
async fn handle_status(
    client_stream: &mut TcpStream,
    handshake: &Handshake,
    kube_server: KubeServer,
) -> Result<(), OpaqueError> {
    tracing::debug!(handshake = ?handshake);
    let client_packet = Packet::parse(client_stream)
        .await
        .ok_or_else(|| "could not parse client_packet".to_string())?;
    if client_packet.id.get_int() != 0 {
        return Err(OpaqueError::create(&format!(
            "Client STATUS: {:#x} Unknown Id -> Shutdown",
            client_packet.id.get_int(),
        )));
    };

    let commit_hash: &'static str = env!("COMMIT_HASH");
    let mut status_struct = StatusStructNew::create();
    status_struct.version.protocol = handshake.protocol_version.get_int();
    let status = kube_server.get_server_status().await?;
    tracing::info!(status = ?status, "status request");
    match status {
        ServerDeploymentStatus::Connectable(mut server_stream) => {
            return kube_server
                .proxy_status(handshake, &client_packet, client_stream, &mut server_stream)
                .await
        }
        ServerDeploymentStatus::Starting | ServerDeploymentStatus::PodOk => {
            status_struct.players.max = 1;
            status_struct.players.online = 1;
            status_struct.description.text = format!("§aServer is starting...§r please wait\n - §dTami§r with §d<3§r §8(rev: {commit_hash})§r");
        }
        ServerDeploymentStatus::Offline => {
            status_struct.players.max = 1;
            status_struct.description.text = format!("Server is currently §onot§r running. \n§aJoin to start it!§r - §dTami§r with §d<3§r §8(rev: {commit_hash})§r");
        }
    };
    let status_res =
        packets::clientbound::status::StatusResponse::set_json(Box::new(status_struct)).await;
    status_res
        .send_packet(client_stream)
        .await
        .map_err(|_| "Failed to send status packet")?;

    mc_server::handle_ping(client_stream).await?;

    Ok(())
}

#[tracing::instrument(level = "info", fields(server_addr = kube_server.get_server_addr()),skip(client_stream, handshake, kube_server,cache))]
async fn handle_login(
    client_stream: &mut TcpStream,
    handshake: &Handshake,
    kube_server: KubeServer,
    cache: Arc<Mutex<Cache>>,
) -> Result<(), OpaqueError> {
    match kube_server.get_server_status().await? {
        ServerDeploymentStatus::Connectable(mut server_stream) => {
            // referenced from:
            // https://github.com/hanyu-dev/tokio-splice2/blob/fc47199fffde8946b0acf867d1fa0b2222267a34/examples/proxy.rs
            let io_sl2sr = tokio_splice2::context::SpliceIoCtx::prepare()
                .map_err(|e| format!("tokio_splice2::context::SpliceIoCtx err={}", e.to_string()))?
                .into_io();

            let io_sr2sl = tokio_splice2::context::SpliceIoCtx::prepare()
                .map_err(|e| format!("tokio_splice2::context::SpliceIoCtx err={}", e.to_string()))?
                .into_io();

            handshake
                .send_packet(&mut server_stream)
                .await
                .map_err(|_| "failed to forward handshake packet to minecraft server")?;

            tracing::info!("proxying with splice");
            let traffic = tokio_splice2::io::SpliceBidiIo { io_sl2sr, io_sr2sl }
                .execute(client_stream, &mut server_stream)
                .await;

            tracing::debug!("data exchanged: tx: {} rx: {}", traffic.tx, traffic.rx);
            if let Some(e) = traffic.error {
                return Err(OpaqueError::create(&format!(
                    "failed to splice; err = {}",
                    e
                )));
            }
        }
        ServerDeploymentStatus::PodOk | ServerDeploymentStatus::Starting => {
            mc_server::send_disconnect(client_stream, "Starting...§d<3§r").await?;
        }
        ServerDeploymentStatus::Offline => {
            kube_server
                .set_scale(cache, 1)
                .map_err(|e| format!("Failed to set depoloyment scale: err = {:?}", e))
                .await?;
            mc_server::send_disconnect(client_stream, "Okayy_starting_it...§d<3§r").await?;
        }
    }
    Ok(())
}
