use serde::{Deserialize, Serialize};

// NOTE u128 not supported in msgpack

pub const MAX_MESSAGE_LENGTH: usize = 512;

pub const MAX_NICK_LENGTH: usize = 20;

#[derive(Deserialize, Serialize, Debug)]
pub enum Command {
    Message {
        contents: String,
        nick: String,
        timestamp: u64,
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
            Command::Message {
                contents,
                nick,
                timestamp: _,
            } => {
                // TODO validate timestamp?
                contents.len() <= MAX_MESSAGE_LENGTH
                    && nick.len() <= MAX_NICK_LENGTH
            }
        }
    }
}
