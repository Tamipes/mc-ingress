use tokio::{io::AsyncWriteExt, net::TcpStream};

use crate::{
    packets::{Packet, SendPacket},
    types::VarString,
};

/// id: 0x00
#[derive(Debug)]
pub struct Disconnect {
    reason: VarString,
    all: Vec<u8>,
}

impl Disconnect {
    pub async fn parse(packet: Packet) -> Option<Disconnect> {
        Some(Disconnect {
            all: packet.all,
            reason: VarString::parse(&mut packet.data.clone()).await?,
        })
    }
    pub fn get_string(&self) -> String {
        self.reason.get_value()
    }
    pub async fn set_reason(reason: String) -> Option<Disconnect> {
        let vec = VarString::from(reason).move_data()?;
        Disconnect::parse(Packet::from_bytes(0, vec)?).await
    }
    pub fn get_all(&self) -> Vec<u8> {
        self.all.clone()
    }
}

impl SendPacket for Disconnect {
    async fn send_packet(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        stream.write_all(&self.all).await?;
        stream.flush().await?;
        Ok(())
    }
}
