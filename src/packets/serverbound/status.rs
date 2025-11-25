use std::io::Write;

use tokio::{io::AsyncWriteExt, net::TcpStream};

use crate::packets::{Packet, SendPacket};

/// id: 0x00
pub struct StatusRequest {
    all: Vec<u8>,
}

impl StatusRequest {
    pub fn parse(packet: Packet) -> Option<StatusRequest> {
        Some(StatusRequest { all: packet.all })
    }
}

impl SendPacket for StatusRequest {
    async fn send_packet(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        stream.write_all(&self.all).await?;
        stream.flush().await?;
        Ok(())
    }
}
