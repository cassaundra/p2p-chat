use serde::{Deserialize, Serialize};

// NOTE u128 not supported in msgpack

#[derive(Deserialize, Serialize, Debug)]
pub enum Command {
    Message { contents: String, timestamp: u64 },
}

// TODO map err

impl Command {
    pub fn decode(encoded: &[u8]) -> crate::Result<Self> {
        // TODO buff?
        let dec = rmp_serde::from_read(encoded)?;
        Ok(dec)
    }

    pub fn encode(&self) -> crate::Result<Vec<u8>> {
        let enc = rmp_serde::to_vec(self)?;
        Ok(enc)
    }
}
