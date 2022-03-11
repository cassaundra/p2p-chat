use std::io;

use libp2p::noise;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("noise protocol error")]
    NoiseError(#[from] noise::NoiseError),
    #[error("libp2p dial error")]
    DialError(#[from] libp2p::swarm::DialError),
    #[error("gossipsub publish error")]
    PublishError(#[from] libp2p::gossipsub::error::PublishError),
    #[error("gossipsub subscription error")]
    SubscriptionError(#[from] libp2p::gossipsub::error::SubscriptionError),
    #[error("kademlia store error")]
    KademeliaStoreError(#[from] libp2p::kad::record::store::Error),
    #[error("signing error")]
    SigningError(#[from] libp2p::identity::error::SigningError),
    #[error("signed envelope decoding error")]
    SignedEnvelopeDecodeError(#[from] libp2p::core::signed_envelope::DecodingError),
    #[error("signed envelope read payload error")]
    SignedEnvelopeReadPayloadError(#[from] libp2p::core::signed_envelope::ReadPayloadError),
    #[error("libp2p transport error")]
    TransportError(#[from] libp2p::TransportError<io::Error>),
    #[error("encode error")]
    EncodeError(#[from] rmp_serde::encode::Error),
    #[error("decode error")]
    DecodeError(#[from] rmp_serde::decode::Error),
    #[error("I/O error")]
    IoError(#[from] io::Error),
    #[error("invalid data received from peer")]
    InvalidData(String),
    #[error("signature mismatch")]
    SignatureMismatch,
}
