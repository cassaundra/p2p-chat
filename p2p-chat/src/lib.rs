use std::{pin::Pin, task::Poll};

use futures::Stream;
use libp2p::{
    core::{either::EitherError, upgrade},
    floodsub::{self, Floodsub, FloodsubConfig, FloodsubEvent},
    identity::Keypair,
    mdns::{Mdns, MdnsEvent},
    mplex,
    noise::{self, AuthenticKeypair, X25519Spec},
    swarm::{ProtocolsHandlerUpgrErr, SwarmBuilder, SwarmEvent},
    tcp::TokioTcpConfig,
    Multiaddr, NetworkBehaviour, PeerId, Swarm, Transport,
};
use log::info;

mod error;
pub use error::*;

pub const TOPIC: &'static str = "p2p-chat";

// TODO message/command protocol

#[derive(Debug)]
enum ComposedEvent {
    Floodsub(FloodsubEvent),
    Mdns(MdnsEvent),
}

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "ComposedEvent")]
struct ComposedBehaviour {
    floodsub: Floodsub,
    mdns: Mdns,
}

impl From<FloodsubEvent> for ComposedEvent {
    fn from(val: FloodsubEvent) -> Self {
        ComposedEvent::Floodsub(val)
    }
}

impl From<MdnsEvent> for ComposedEvent {
    fn from(val: MdnsEvent) -> Self {
        ComposedEvent::Mdns(val)
    }
}

#[derive(Debug)]
pub enum ClientEvent {
    Message { contents: String, source: PeerId },
}

pub struct Client {
    id_keys: Keypair,
    swarm: Swarm<ComposedBehaviour>,
}

impl Client {
    pub async fn new(id_keys: Keypair) -> crate::Result<Self> {
        let peer_id = PeerId::from(id_keys.public());
        let noise_keys = gen_static_keypair(&id_keys)?;

        let transport = TokioTcpConfig::new()
            .nodelay(false)
            .upgrade(upgrade::Version::V1)
            .authenticate(
                noise::NoiseConfig::xx(noise_keys).into_authenticated(),
            )
            .multiplex(mplex::MplexConfig::new())
            .boxed();

        let topic = floodsub::Topic::new(TOPIC);

        let swarm = {
            let mut behaviour = ComposedBehaviour {
                floodsub: Floodsub::from_config(FloodsubConfig {
                    local_peer_id: peer_id,
                    subscribe_local_messages: true,
                }),
                mdns: Mdns::new(Default::default()).await?,
            };

            behaviour.floodsub.subscribe(topic);

            SwarmBuilder::new(transport, behaviour, peer_id)
                .executor(Box::new(|fut| {
                    tokio::spawn(fut);
                }))
                .build()
        };

        Ok(Client { id_keys, swarm })
    }

    pub fn send_message(&mut self, message: &str) {
        self.swarm
            .behaviour_mut()
            .floodsub
            .publish(floodsub::Topic::new(TOPIC), message.as_bytes());
    }

    pub fn dial(&mut self, addr: Multiaddr) -> crate::Result<()> {
        info!("Dialing {}", addr);
        self.swarm.dial(addr)?;
        Ok(())
    }

    pub fn listen_on(&mut self, addr: Multiaddr) -> crate::Result<()> {
        info!("Listening on {}", addr);
        self.swarm.listen_on(addr)?;
        Ok(())
    }

    pub fn is_connected(&self, peer_id: &PeerId) -> bool {
        self.swarm.is_connected(peer_id)
    }

    pub fn peer_id(&self) -> PeerId {
        PeerId::from(self.id_keys.public())
    }

    fn handle_event<UpgradeErr, OtherErr>(
        &mut self,
        event: SwarmEvent<
            ComposedEvent,
            EitherError<ProtocolsHandlerUpgrErr<UpgradeErr>, OtherErr>,
        >,
    ) -> Option<ClientEvent> {
        match event {
            SwarmEvent::Behaviour(ComposedEvent::Floodsub(event)) => {
                match event {
                    FloodsubEvent::Message(message) => {
                        return Some(ClientEvent::Message {
                            contents: String::from_utf8_lossy(&message.data)
                                .to_string(),
                            source: message.source,
                        });
                    }
                    FloodsubEvent::Subscribed {
                        peer_id: _,
                        topic: _,
                    } => {}
                    FloodsubEvent::Unsubscribed {
                        peer_id: _,
                        topic: _,
                    } => {}
                }
            }
            SwarmEvent::Behaviour(ComposedEvent::Mdns(event)) => match event {
                MdnsEvent::Discovered(list) => {
                    for (peer, _) in list {
                        self.swarm
                            .behaviour_mut()
                            .floodsub
                            .add_node_to_partial_view(peer);
                    }
                }
                MdnsEvent::Expired(list) => {
                    for (peer, _) in list {
                        let behaviour = self.swarm.behaviour_mut();
                        if !behaviour.mdns.has_node(&peer) {
                            behaviour
                                .floodsub
                                .remove_node_from_partial_view(&peer);
                        }
                    }
                }
            },
            _ => {} // TODO log some of rest?
        }

        None
    }
}

impl Stream for Client {
    // TODO this should actually be a bare ClientEvent
    type Item = Option<ClientEvent>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.swarm)
            .poll_next(cx)
            .map(|e| e.map(|e| self.handle_event(e)))
    }
}

// TODO seeds

pub fn gen_id_keys() -> Keypair {
    Keypair::generate_ed25519()
}

pub fn gen_static_keypair(
    id_keys: &Keypair,
) -> Result<AuthenticKeypair<X25519Spec>> {
    Ok(noise::Keypair::<noise::X25519Spec>::new().into_authentic(id_keys)?)
}

const NUM_NAMES: usize = 4;
const NAMES: [&'static str; NUM_NAMES] =
    ["alice", "bailie", "charlotte", "danielle"];

pub fn name_from_peer(peer_id: PeerId) -> &'static str {
    // obviously not very smart or "secure"
    let sum = peer_id
        .to_bytes()
        .iter()
        .fold(0u8, |acc, b| acc.wrapping_add(*b));
    NAMES[sum as usize % NUM_NAMES]
}
