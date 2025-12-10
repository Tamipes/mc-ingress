use tokio::{io::AsyncWriteExt, net::TcpStream};

use crate::{
    packets::{serverbound::handshake::Handshake, Packet, SendPacket},
    OpaqueError,
};

#[tracing::instrument(skip(client_stream))]
pub async fn handle_ping(client_stream: &mut TcpStream) -> Result<(), OpaqueError> {
    // --- Respond to ping packet ---
    let ping_packet = Packet::parse(client_stream).await?;
    match ping_packet.id.get_int() {
        1 => Ok(ping_packet
            .send_packet(client_stream)
            .await
            .map_err(|_| "Failed to send ping")?),
        _ => Err(OpaqueError::create(&format!(
            "Expected ping packet, got: {}",
            ping_packet.id.get_int()
        ))),
    }
}

/// Disconnects the client.
///
/// It works if the client is in the login state, and it
/// has *already* and *only* sent the handshake packet.
#[tracing::instrument(skip(client_stream))]
pub async fn send_disconnect(
    client_stream: &mut TcpStream,
    reason: &str,
) -> Result<(), OpaqueError> {
    let _client_packet = Packet::parse(client_stream).await?;

    let disconnect_packet =
        crate::packets::clientbound::login::Disconnect::set_reason(reason.to_owned())
            .await
            .ok_or_else(|| "failed to *create* disconnect packet")?;
    disconnect_packet
        .send_packet(client_stream)
        .await
        .map_err(|_| "failed to *send* disconnect packet")?;
    client_stream.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

pub trait MinecraftServerHandle {
    async fn start(&self) -> Result<(), OpaqueError>;
    async fn stop(&self) -> Result<(), OpaqueError>;
    async fn query_status(&self) -> Result<ServerDeploymentStatus, OpaqueError>;
    fn get_internal_address(&self) -> Option<String>;
    fn get_addr(&self) -> Option<String>;

    async fn query_server_connectable(&self) -> Result<TcpStream, OpaqueError> {
        let port = self
            .get_internal_address()
            .ok_or_else(|| "failed to get internal address from server")?;
        let server_stream = TcpStream::connect(port)
            .await
            .map_err(|_| "failed to connect to minecraft server")?;

        tracing::trace!(
            "successfully connected to backend server; (connectibility check) {:?}",
            server_stream.peer_addr()
        );
        Ok(server_stream)
    }

    /// Takes the already received packets, sends them to the server,
    /// and proceeds to proxy the rest of the connection transparently.
    async fn proxy_status(
        &self,
        handshake: &Handshake,
        status_request: &Packet,
        client_stream: &mut TcpStream,
        server_stream: &mut TcpStream,
    ) -> Result<(), OpaqueError> {
        handshake
            .send_packet(server_stream)
            .await
            .map_err(|_| "failed to forward handshake packet to minecraft server")?;
        status_request
            .send_packet(server_stream)
            .await
            .map_err(|_| "failed to forward status request packet to minecraft server")?;

        let data_amount = tokio::io::copy_bidirectional(client_stream, server_stream)
            .await
            .map_err(|e| {
                format!(
                    "error during bidirectional copy between server and client; err={:?}",
                    e
                )
            })?;
        tracing::trace!("data exchanged while proxying status: {:?}", data_amount);
        Ok(())
    }
}

pub trait MinecraftAPI<T> {
    async fn query_server(&self, addr: String) -> Result<T, OpaqueError>;
}

pub enum ServerDeploymentStatus {
    Connectable(TcpStream),
    Starting,
    PodOk,
    Offline,
}
