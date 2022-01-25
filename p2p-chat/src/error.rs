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
    #[error("libp2p transport error")]
    TransportError(#[from] libp2p::TransportError<io::Error>),
    #[error("I/O error")]
    IoError(#[from] io::Error),
}
