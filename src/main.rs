use std::env;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tracing_subscriber::{prelude::*, EnvFilter};

use crate::mc_server::{MinecraftAPI, MinecraftServerHandle, ServerDeploymentStatus};
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
    let filter_layer = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(filter_layer)
        .with(tracing_error::ErrorLayer::default())
        .init();
    tracing::info!("mc-ingress");

    let revision: &'static str = env!("COMMIT_HASH");
    tracing::info!(revision);

    let api = kube_cache::McApi::create().unwrap();
    tracing::info!("initialized kube api");

    let addr = match env::var("BIND_ADDR") {
        Ok(x) => x,
        Err(_) => "0.0.0.0:25565".to_string(),
    };
    let listener = TcpListener::bind(addr.clone()).await.unwrap();
    tracing::info!(addr, "started tcp server");

    loop {
        let (socket, addr) = listener.accept().await.unwrap();
        let api = api.clone();

        tokio::spawn(async move {
            tracing::debug!(
                addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
                "Client connected"
            );
            if let Err(e) = process_connection(socket, addr, api).await {
                tracing::error!(
                    // addr = format!("{}:{}", addr.ip().to_string(), addr.port().to_string()),
                    trace = format!("{}", e.get_span_trace()),
                    err = format!("{}", e.context),
                    "Client disconnected"
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

#[tracing::instrument(level = "info", skip(api, client_stream))]
async fn process_connection<T: MinecraftServerHandle>(
    mut client_stream: TcpStream,
    addr: SocketAddr,
    api: impl MinecraftAPI<T>,
) -> Result<(), OpaqueError> {
    let client_packet = Packet::parse(&mut client_stream).await?;

    // --- Handshake ---
    let handshake;
    let next_server_state;
    let packet_id = client_packet.id.get_int();
    if packet_id != 0 {
        return Err(OpaqueError::create(&format!(
            "Client HANDSHAKE -> bad packet; id={packet_id} Disconnecting..."
        )));
    }
    handshake = packets::serverbound::handshake::Handshake::parse(client_packet)
        .await
        .ok_or_else(|| "Client HANDSHAKE -> malformed packet; Disconnecting...".to_string())?;

    next_server_state = handshake.get_next_state();

    match next_server_state {
        packets::ProtocolState::Status => {
            handle_status(&mut client_stream, &handshake, api).await?;
        }
        packets::ProtocolState::Login => handle_login(&mut client_stream, &handshake, api).await?,
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

#[tracing::instrument(level = "info", fields(server_addr = handshake.get_server_address()),skip(client_stream, handshake, api))]
async fn handle_status<T: MinecraftServerHandle>(
    client_stream: &mut TcpStream,
    handshake: &Handshake,
    api: impl MinecraftAPI<T>,
) -> Result<(), OpaqueError> {
    let client_packet = Packet::parse(client_stream).await?;
    if client_packet.id.get_int() != 0 {
        return Err(OpaqueError::create(&format!(
            "Client STATUS: {:#x} Unknown Id -> Shutdown",
            client_packet.id.get_int(),
        )));
    };

    let server_addr = handshake.get_server_address();
    let commit_hash: &'static str = env!("COMMIT_HASH");
    let mut status_struct = StatusStructNew::create();
    status_struct.version.protocol = handshake.protocol_version.get_int();
    let bye_message = format!(" - §dTami§r with §d<3§r §8(rev: {commit_hash})§r");

    let server = match api.query_server(&handshake.get_server_address()).await {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!(err = e.context);
            status_struct.players.max = 0;
            status_struct.players.online = 0;
            status_struct.description.text = format!(
                "Could not find §kserver§r: §f§o{server_addr}§r\nMinecraft Ingress{bye_message}"
            );

            mc_server::complete_status_request(client_stream, status_struct).await?;

            // Recieve the ping packet, so the client does not send it again
            let _ping = Packet::parse(client_stream).await?;
            // Send a bad ping packet back, so the client shows *searching* icon
            let _pong = Packet::new(9, vec![0; 8])
                .ok_or("failed to create empty pong packet?")?
                .send_packet(client_stream)
                .await
                .map_err(|_| "failed to send pong packet")?;

            return Ok(());
        }
    };
    tracing::debug!("kube server status: {:?}", server.query_status().await?);
    let status = server.query_status().await?;
    tracing::info!(status = ?status, "status request");
    match status {
        ServerDeploymentStatus::Connectable(mut server_stream) => {
            return server
                .proxy_status(handshake, &client_packet, client_stream, &mut server_stream)
                .await
        }
        ServerDeploymentStatus::Starting | ServerDeploymentStatus::PodOk => {
            status_struct.players.max = 1;
            status_struct.players.online = 1;
            status_struct.description.text =
                format!("§aServer is starting...§r please wait\n{bye_message}");
        }
        ServerDeploymentStatus::Offline => {
            status_struct.players.max = 1;
            status_struct.description.text = format!(
                "Server is currently §onot§r running. \n§aJoin to start it!§r    {bye_message}"
            );
        }
    };

    mc_server::complete_status_request(client_stream, status_struct).await?;
    return mc_server::handle_ping(client_stream).await;
}

#[tracing::instrument(level = "info", fields(server_addr = handshake.get_server_address()),skip(client_stream, handshake, api))]
async fn handle_login<T: MinecraftServerHandle>(
    client_stream: &mut TcpStream,
    handshake: &Handshake,
    api: impl MinecraftAPI<T>,
) -> Result<(), OpaqueError> {
    let addr = handshake.get_server_address();
    // Thanks to a buggy minecraft, when the client sends a join
    // from a SRV DNS record, it will not use the address typed
    // in the game, but use the address redicted *to* by the
    // DNS record as the address for joining, plus a trailing "."
    //
    // For example:
    // server.example.com (_minecraft._tcp.server.example.com)
    // (the typed address)     I (the DNS SRV record which gets read)
    //                         V
    //            5 25565 server.example.com
    //                         I (the response for the DNS SRV query)
    //                         V
    //                server.example.com.
    //         (the address used in the protocol)
    let server = api.query_server(addr.trim_end_matches(".")).await?;

    let status = server.query_status().await?;
    tracing::debug!(msg = "server status", status = ?status);
    match status {
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
            server.start().await?;
            api.start_watch(server.clone(), Duration::from_secs(600))
                .await?;
            mc_server::send_disconnect(client_stream, "Okayy_starting_it...§d<3§r").await?;
        }
    }
    Ok(())
}
