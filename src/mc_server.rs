use tokio::net::TcpStream;

use crate::{
    packets::{Packet, SendPacket},
    OpaqueError,
};

#[tracing::instrument(skip(client_stream))]
pub async fn handle_ping(client_stream: &mut TcpStream) -> Result<(), OpaqueError> {
    // --- Respond to ping packet ---
    let ping_packet = Packet::parse(client_stream)
        .await
        .ok_or("Ping packett failed to parse")?;
    match ping_packet.id.get_int() {
        1 => Ok(ping_packet
            .send_packet(client_stream)
            .await
            .map_err(|_| "Failed to send ping")?),
        _ => {
            return Err(OpaqueError::create(&format!(
                "Expected ping packet, got: {}",
                ping_packet.id.get_int()
            )));
        }
    }
}

// pub async fn handle_redirect(client_stream: &mut TcpStream) -> Result<(), OpaqueError> {}
