use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

// NOTE u128 not supported in msgpack

pub const MAX_MESSAGE_LENGTH: usize = 512;

pub const MAX_NICK_LENGTH: usize = 20;

// TODO ?
pub type ChannelIdentifier = String;

#[derive(Deserialize, Serialize, Debug)]
pub struct Channel {
    identifier: ChannelIdentifier,
    owner: Multiaddr,
    peers: Vec<Multiaddr>,
    version: u64,
}

#[derive(Deserialize, Serialize, Debug)]
pub enum MessageType {
    Normal,
    Me,
}

#[derive(Deserialize, Serialize, Debug)]
pub enum Command {
    ChannelUpdate {
        channel: Channel,
    },
    ChannelRequestJoin {
        channel: ChannelIdentifier,
    },
    ChannelRequestLeave {
        channel: ChannelIdentifier,
    },
    MessageSend {
        contents: String,
        timestamp: u64,
        message_type: MessageType,
    },
    NicknameUpdate {
        nick: String,
    },
}

// TODO map err

impl Command {
    pub fn decode(encoded: &[u8]) -> crate::Result<Self> {
        // TODO buff?
        let dec: Command = rmp_serde::from_read(encoded)?;

        if !dec.is_valid() {
            return Err(crate::Error::InvalidMessage);
        }

        Ok(dec)
    }

    pub fn encode(&self) -> crate::Result<Vec<u8>> {
        let enc = rmp_serde::to_vec(self)?;
        Ok(enc)
    }

    pub fn is_valid(&self) -> bool {
        match self {
            Command::MessageSend {
                contents,
                timestamp: _,
                message_type: _,
            } => {
                // TODO validate timestamp?
                !contents.is_empty() && contents.len() <= MAX_MESSAGE_LENGTH
            }
            Command::NicknameUpdate { nick } => {
                nick.is_empty() && nick.len() <= MAX_NICK_LENGTH
            }
            _ => true,
        }
    }
}
