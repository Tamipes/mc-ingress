// use std::io::{self, Read, Write};

use std::fmt;

use tokio::io;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

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
    #[tracing::instrument(level = "info", skip(stream))]
    pub async fn parse(stream: &mut TcpStream) -> Result<Packet, ParseErrorTrace> {
        let length = VarInt::parse(stream)
            .await
            .ok_or(ParseError::IDParseError)?;

        tracing::trace!(length = length.get_int());
        let id = match VarInt::parse(stream).await {
            Some(x) => x,
            None => {
                return Err(ParseError::IDParseError.into());
            }
        };
        if id.get_int() == 122 {
            return Err(ParseError::WeirdID.into());
        }

        // TODO: investigate this, becuase it is just a hunch
        // but if it is too big, the vec![] macro panics
        let data_size = length.get_int() as usize - id.get_data().len();
        if data_size > u16::MAX.into() {
            return Err(ParseError::LengthIsTooBig(length.get_int()).into());
        }
        if data_size < 0 {
            return Err(ParseError::PacketLengthInvalid(length.get_int()).into());
        }
        // TODO: this is a bandaid fix; the above checks *should* make sure the
        // next line does not run into "capacity overflow", but it doesn't work
        let mut data: Vec<u8> = match std::panic::catch_unwind(|| vec![0; data_size]) {
            Ok(x) => x,
            Err(e) => {
                return Err(ParseError::BufferAllocationPanic(format!(
                    "panic while allocating with vec![] macro len_int={} usize={} len_data={} error={:?}",
                    id.get_data().len(),
                    length.get_int(),
                    length.get_int() as usize - id.get_data().len(),
                    e
                )).into());
            }
        };

        match stream.read_exact(&mut data).await {
            Ok(_) => {
                let mut vec = id.get_data();
                vec.append(&mut data.clone());
                let mut all = length.get_data();
                all.append(&mut vec);
                Ok(Packet {
                    id,
                    length,
                    data,
                    all,
                })
            }
            Err(e) => {
                return Err(ParseError::StreamReadError(format!(
                    "length={}; data={:?}; error={:?}; ",
                    length.get_int(),
                    length.get_data(),
                    e
                ))
                .into());
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
}

pub struct ParseErrorTrace {
    pub inner: ParseError,
    pub context: tracing_error::SpanTrace,
}

impl From<ParseError> for ParseErrorTrace {
    fn from(value: ParseError) -> Self {
        Self {
            inner: value,
            context: tracing_error::SpanTrace::capture(),
        }
    }
}

pub enum ParseError {
    IDParseError,
    WeirdID,
    LengthIsTooBig(i32),
    DataIsZero,
    PacketLengthInvalid(i32),
    StreamReadError(String),
    BufferAllocationPanic(String),
}

impl fmt::Debug for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IDParseError => write!(f, "IDParseError: could not parse packet id"),
            Self::WeirdID => write!(f, "WeirdID: weird packet id encountered: 122"),
            Self::LengthIsTooBig(x) => {
                write!(f, "LengthIsTooBig: packet length is too big; len={x}")
            }
            Self::DataIsZero => write!(f, "DataIsZero: data portion of packet does not exist"),
            Self::PacketLengthInvalid(x) => write!(
                f,
                "PacketLengthInvalid: packet lenght is smaller then packet len={x}"
            ),
            Self::StreamReadError(str) => write!(f, "StreamReadError: {str}"),
            Self::BufferAllocationPanic(str) => write!(f, "BufferAllocationPanic: {str}"),
        }
    }
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
