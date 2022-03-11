use libp2p::{core::SignedEnvelope, gossipsub, identity::Keypair, PeerId};
use serde::{Deserialize, Serialize};

// NOTE u128 not supported in msgpack

/// The gossipsub topic for top-level network communication.
pub const DEFAULT_GOSSIPSUB_TOPIC: &str = "/p2p-chat";

/// The signed envelope domain for use in Kademlia memory store.
pub const SIGNED_ENVELOPE_DOMAIN: &str = "p2p-chat-data";

/// The maximum length of a message, in characters.
pub const MAX_MESSAGE_LENGTH: usize = 512;

/// The maximum length of a channel identifier, in characters.
pub const MAX_CHANNEL_IDENTIFIER_LENGTH: usize = 64;

/// The maximum length of a nickname, in characters.
pub const MAX_NICK_LENGTH: usize = 20;

/// The MessagePack codec code within [multicodec](https://github.com/multiformats/multicodec).
pub const MULTICODEC_MSGPACK: &[u8] = &[0x02, 0x01];

// TODO ?
pub type ChannelIdentifier = String;

#[derive(Deserialize, Serialize, Debug)]
pub struct Channel {
    identifier: ChannelIdentifier,
    owner: PeerId,
    peers: Vec<PeerId>,
    version: u64,
}

#[derive(Deserialize, Serialize, Copy, Clone, Debug)]
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
        channel: ChannelIdentifier,
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
            return Err(crate::Error::InvalidData(String::from(
                "decoded command not valid",
            )));
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
                channel,
                timestamp: _,
                message_type: _,
            } => {
                // TODO validate timestamp?
                !contents.is_empty()
                    && contents.len() <= MAX_MESSAGE_LENGTH
                    && channel.len() <= MAX_CHANNEL_IDENTIFIER_LENGTH
            }
            Command::NicknameUpdate { nick } => {
                nick.is_empty() && nick.len() <= MAX_NICK_LENGTH
            }
            _ => true,
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub enum MemoryKey {
    Nickname(PeerId),
    Channel(String),
}

impl MemoryKey {
    pub fn decode(encoded: &[u8]) -> crate::Result<Self> {
        Ok(rmp_serde::from_read(encoded)?)
    }

    pub fn encode(&self) -> crate::Result<Vec<u8>> {
        Ok(rmp_serde::to_vec(self)?)
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub enum MemoryValue {
    Nickname { user: PeerId, nickname: String },
    Channel(Channel),
}

impl MemoryValue {
    pub fn decode(encoded: &[u8]) -> crate::Result<Self> {
        let envelope = SignedEnvelope::from_protobuf_encoding(encoded)?;
        let (payload, signing_key) = envelope.payload_and_signing_key(
            SIGNED_ENVELOPE_DOMAIN.to_owned(),
            MULTICODEC_MSGPACK,
        )?;
        let value = rmp_serde::from_read(payload)?;

        let expected_signer = match &value {
            MemoryValue::Nickname { user, .. } => user,
            MemoryValue::Channel(channel) => &channel.owner,
        };

        if expected_signer != &signing_key.to_peer_id() {
            return Err(crate::Error::SignatureMismatch);
        }

        Ok(value)
    }

    pub fn encode_signed(&self, key: &Keypair) -> crate::Result<Vec<u8>> {
        let payload = rmp_serde::to_vec(self)?;
        let envelope = SignedEnvelope::new(
            key,
            SIGNED_ENVELOPE_DOMAIN.to_owned(),
            MULTICODEC_MSGPACK.to_owned(),
            payload,
        )?;
        Ok(envelope.into_protobuf_encoding())
    }
}

pub fn topic_from_channel(ident: &ChannelIdentifier) -> gossipsub::IdentTopic {
    gossipsub::IdentTopic::new(format!("/p2p-chat/channel/{ident}"))
}
