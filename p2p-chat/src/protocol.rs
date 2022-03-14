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
pub const MAX_CHANNEL_IDENTIFIER_LENGTH: usize = 20;

/// The maximum length of a nickname, in characters.
pub const MAX_NICK_LENGTH: usize = 20;

/// The MessagePack codec code within [multicodec](https://github.com/multiformats/multicodec).
pub const MULTICODEC_MSGPACK: &[u8] = &[0x02, 0x01];

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

        dec.check_valid()?;

        Ok(dec)
    }

    pub fn encode(&self) -> crate::Result<Vec<u8>> {
        self.check_valid()?;
        Ok(rmp_serde::to_vec(self)?)
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
                    && !channel.is_empty()
                    && channel.len() <= MAX_CHANNEL_IDENTIFIER_LENGTH
            }
            Command::NicknameUpdate { nick } => {
                !nick.is_empty() && nick.len() <= MAX_NICK_LENGTH
            }
            _ => true,
        }
    }

    fn check_valid(&self) -> crate::Result<()> {
        if !self.is_valid() {
            Err(crate::Error::InvalidData(String::from(
                "command is not valid",
            )))
        } else {
            Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_message_send() {
        // good
        assert!(Command::MessageSend {
            contents: "hello world!".to_owned(),
            channel: "hello".to_owned(),
            timestamp: 64,
            message_type: MessageType::Normal
        }
        .is_valid());

        // bad: too long of channel name
        assert!(!Command::MessageSend {
            contents: "hello world!".to_owned(),
            channel: "thisisanabsurdlylongchannelnameohnoiwonderifthisisokay"
                .to_owned(),
            timestamp: 64,
            message_type: MessageType::Normal
        }
        .is_valid());

        // bad: empty channel name
        assert!(!Command::MessageSend {
            contents: "hello world!".to_owned(),
            channel: String::new(),
            timestamp: 64,
            message_type: MessageType::Normal
        }
        .is_valid());

        // bad: very long message
        assert!(!Command::MessageSend {
            contents: String::new(),
            channel: "hello".repeat(200),
            timestamp: 0,
            message_type: MessageType::Normal
        }
        .is_valid());

        // bad: empty message
        assert!(!Command::MessageSend {
            contents: String::new(),
            channel: "hello".to_owned(),
            timestamp: 0,
            message_type: MessageType::Me
        }
        .is_valid());
    }

    #[test]
    fn test_command_nickname_update() {
        // good
        assert!(Command::NicknameUpdate {
            nick: "alice".to_owned()
        }
        .is_valid());

        // bad: name empty
        assert!(!Command::NicknameUpdate {
            nick: String::new(),
        }
        .is_valid());

        // bad: name too long
        assert!(!Command::NicknameUpdate {
            nick: "verylongnameverylongname".to_owned()
        }
        .is_valid());
    }

    #[test]
    fn test_channel_topics() {
        assert_eq!(
            "/p2p-chat/channel/hello world",
            &format!("{}", topic_from_channel(&"hello world".to_owned()))
        )
    }
}
