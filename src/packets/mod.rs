// use std::io::{self, Read, Write};

use std::fmt;

use tokio::io;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tracing::instrument;

use crate::types::VarInt;
pub mod clientbound;
pub mod serverbound;

#[derive(Debug)]
pub struct Packet {
    pub id: VarInt,
    length: VarInt,
    pub data: Vec<u8>,
    pub all: Vec<u8>,
}
pub trait SendPacket {
    async fn send_packet(&self, stream: &mut TcpStream) -> io::Result<()>;
}

impl SendPacket for Packet {
    async fn send_packet(&self, stream: &mut TcpStream) -> io::Result<()> {
        stream.write_all(&self.all).await?;
        Ok(())
    }
}

impl Packet {
    pub fn from_bytes(id: i32, data: Vec<u8>) -> Option<Packet> {
        let id = VarInt::from(id)?;
        let length = VarInt::from((data.len() + id.get_data().len()) as i32)?;
        let mut all = length.get_data();
        all.append(&mut id.get_data());
        all.append(&mut data.clone());
        Some(Packet {
            id,
            length,
            data,
            all,
        })
    }
    pub fn new(id: i32, data: Vec<u8>) -> Option<Packet> {
        let mut vec = VarInt::from(id)?.get_data();
        vec.append(&mut data.clone());

        let mut all = VarInt::from(vec.len() as i32)?.get_data();
        all.append(&mut vec.clone());
        all.append(&mut data.clone());
        Some(Packet {
            id: VarInt::from(id)?,
            length: VarInt::from(vec.len() as i32)?,
            data,
            all,
        })
    }
    #[instrument(level = "info",skip(buf),fields(addr = buf.peer_addr().map(|x| x.to_string()).unwrap_or("unknown".to_string())))]
    pub async fn parse(buf: &mut TcpStream) -> Option<Packet> {
        let length = VarInt::parse(buf).await?;

        tracing::trace!(length = length.get_int());
        let id = match VarInt::parse(buf).await {
            Some(x) => x,
            None => {
                tracing::error!("could not parse packet id");
                return None;
            }
        };
        if id.get_int() == 122 {
            tracing::warn!("weird packet id encountered: 122");
            return None;
        }

        // TODO: investigate this, becuase it is just a hunch
        // but if it is too big, the vec![] macro panics
        if length.get_int() > u16::MAX.into() {
            tracing::error!(len = length.get_int(), "packet length is too big");
            return None;
        }
        // TODO: this is a bandaid fix; the above check *should* make sure the
        // next line does not run into "capacity overflow", but it doesn't work
        let mut data: Vec<u8> = match std::panic::catch_unwind(|| {
            vec![0; length.get_int() as usize - id.get_data().len()]
        }) {
            Ok(x) => x,
            Err(e) => {
                tracing::error!(
                    len_int = length.get_int(),
                    usize = length.get_int() as usize - id.get_data().len(),
                    len_data = id.get_data().len(),
                    error = ?e,
                    "panic while allocating with vec![] macro"
                );
                return None;
            }
        };

        match buf.read_exact(&mut data).await {
            Ok(_) => {
                // data_id.append(&mut data.clone());
                // data_length.append(&mut data_id);
                let mut vec = id.get_data();
                vec.append(&mut data.clone());
                let mut all = length.get_data();
                all.append(&mut vec);
                Some(Packet {
                    id,
                    length,
                    data,
                    all,
                })
            }
            Err(e) => {
                tracing::error!(length = length.get_int(), data = ?length.get_data(),error = e.to_string(),"buffer read error");
                return None;
            }
        }
    }
    pub fn all(&self) -> Option<Vec<u8>> {
        let mut vec = self.id.get_data();
        vec.append(&mut self.data.clone());
        let mut all = VarInt::from(vec.len() as i32)?.get_data();
        all.append(&mut vec);
        return Some(all);
    }
    // pub fn proto_name(&self, state: &Type) -> String {
    //     match state {
    //         ProtocolState::Handshaking => match self.id.get_int() {
    //             0 => "Handshake".to_owned(),
    //             _ => "error".to_owned(),
    //         },
    //         ProtocolState::Status => match self.id.get_int() {
    //             0 => "StatusRequest".to_owned(),
    //             1 => "PingRequest".to_owned(),
    //             _ => "error".to_owned(),
    //         },
    //         _ => "Dont care state".to_owned(),
    //     }
    // }
}

#[derive(Copy, Clone, PartialEq)]
pub enum ProtocolState {
    Handshaking,
    Status,
    Login,
    Transfer,
    Configuration,
    Play,
    ShutDown,
}

impl ToString for ProtocolState {
    fn to_string(&self) -> String {
        match self {
            ProtocolState::Handshaking => "Hanshake",
            ProtocolState::Status => "Status",
            ProtocolState::Login => "Login",
            ProtocolState::Configuration => "Configuration ",
            ProtocolState::Play => "Play",
            ProtocolState::ShutDown => "Shutdown",
            ProtocolState::Transfer => "Transfer",
        }
        .to_string()
    }
}
