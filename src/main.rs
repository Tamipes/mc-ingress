//! This is a simple imitation of the basic functionality of kubectl:
//! kubectl {get, delete, apply, watch, edit} <resource> [name]
//! with labels and namespace selectors supported.
use std::{net::SocketAddr, sync::Arc};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing_subscriber::{prelude::*, EnvFilter};

use crate::kube_cache::{KubeServer, ServerDeploymentStatus};
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
    // tracing_subscriber::fmt::init();
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
    tracing::info!("COMMIT_HASH: {}", commit_hash);

    let cache = kube_cache::Cache::create().unwrap();
    let arc_cache = Arc::new(Mutex::new(cache));
    tracing::info!("kube api initialized");

    let listener = TcpListener::bind("0.0.0.0:25565").await.unwrap();
    tracing::info!("tcp server started");

    loop {
        let (socket, addr) = listener.accept().await.unwrap();
        let acc = arc_cache.clone();

        tokio::spawn(async move {
            tracing::info!(
                addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
                "Client connected"
            );
            if let Err(e) = process_connection(socket, addr, acc).await {
                tracing::error!(
                    message = format!("Client disconnected"),
                    addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
                    trace = format!("{}", e.get_span_trace()),
                    err = format!("{}", e.context)
                );
            } else {
                tracing::info!(
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
            tracing::trace!("Client HANDSHAKE -> bad packet; Disconnecting...");
            return Ok(());
        }
    };

    // --- Handshake ---
    let handshake;
    let server_state;
    if client_packet.id.get_int() != 0 {
        return Err(OpaqueError::create(
            "Client HANDSHAKE -> bad packet; Disconnecting...",
        ));
    }
    handshake = packets::serverbound::handshake::Handshake::parse(client_packet)
        .await
        .ok_or_else(|| "Handshake request from client failed to parse".to_string())?;

    server_state = handshake.get_next_state();

    match server_state {
        packets::ProtocolState::Status => {
            handle_status(
                &mut client_stream,
                cache,
                handshake.get_server_address(),
                &handshake,
            )
            .await?;
        }
        packets::ProtocolState::Login => todo!(),
        packets::ProtocolState::Transfer => todo!(),
        _ => todo!(),
    };
    Ok(())
}

#[tracing::instrument(level = "info", skip(client_stream, cache, handshake))]
async fn handle_status(
    client_stream: &mut TcpStream,
    cache: Arc<Mutex<kube_cache::Cache>>,
    server_addr: String,
    handshake: &Handshake,
) -> Result<(), OpaqueError> {
    tracing::debug!(handshake = ?handshake);
    let client_packet = Packet::parse(client_stream)
        .await
        .ok_or_else(|| "Could not parse client_packet".to_string())?;
    match client_packet.id.get_int() {
        0 => tracing::info!("Client STATUS: {:#x} Status Request", 0),
        _ => {
            return Err(OpaqueError::create(&format!(
                "Client STATUS: {:#x} Unknown Id -> Shutdown",
                client_packet.id.get_int(),
            )))
        }
    };

    let kube_server = KubeServer::create(cache, &server_addr).await?;

    let status: ServerDeploymentStatus = kube_server.get_server_status().await?;
    tracing::info!("kube server status: {:?}", status);

    let commit_hash: &'static str = env!("COMMIT_HASH");
    let mut status_struct = StatusStructNew::create();
    status_struct.version.protocol = handshake.protocol_version.get_int();
    match status {
        ServerDeploymentStatus::Connectable => {
            return kube_server
                .proxy_status(handshake, &client_packet, client_stream)
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
