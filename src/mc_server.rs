use tokio::{io::AsyncWriteExt, net::TcpStream};

use crate::{
    packets::{Packet, SendPacket},
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
    todo!()
}
