use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    pin::Pin,
    task::Poll,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::Stream;
use libp2p::{
    core::{either::EitherError, upgrade},
    gossipsub::{
        self, error::GossipsubHandlerError, Gossipsub, GossipsubEvent,
    },
    identity::Keypair,
    mdns::{Mdns, MdnsEvent},
    mplex,
    noise::{self, AuthenticKeypair, X25519Spec},
    swarm::{SwarmBuilder, SwarmEvent},
    tcp::TokioTcpConfig,
    Multiaddr, NetworkBehaviour, PeerId, Swarm, Transport,
};
use log::{info, warn};

use crate::protocol::Command;

pub const TOPIC: &str = "p2p-chat";

// TODO message/command protocol

#[derive(Debug)]
enum ComposedEvent {
    Gossipsub(GossipsubEvent),
    Mdns(MdnsEvent),
}

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "ComposedEvent")]
struct ComposedBehaviour {
    gossipsub: Gossipsub,
    mdns: Mdns,
}

impl From<GossipsubEvent> for ComposedEvent {
    fn from(val: GossipsubEvent) -> Self {
        ComposedEvent::Gossipsub(val)
    }
}

impl From<MdnsEvent> for ComposedEvent {
    fn from(val: MdnsEvent) -> Self {
        ComposedEvent::Mdns(val)
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum ClientEvent {
    Message { contents: String, source: PeerId },
    PeerConnected { source: PeerId },
    PeerDisconnected { source: PeerId },
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

        let topic = gossipsub::IdentTopic::new(TOPIC);

        let swarm = {
            let gossipsub_config = gossipsub::GossipsubConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(15))
                .validation_mode(gossipsub::ValidationMode::Strict)
                .message_id_fn(message_id_fn)
                .allow_self_origin(true)
                .build()
                .unwrap();

            let mut behaviour = ComposedBehaviour {
                gossipsub: Gossipsub::new(
                    gossipsub::MessageAuthenticity::Signed(id_keys.clone()),
                    gossipsub_config,
                )
                .unwrap(),
                mdns: Mdns::new(Default::default()).await?,
            };

            behaviour.gossipsub.subscribe(&topic)?;

            SwarmBuilder::new(transport, behaviour, peer_id)
                .executor(Box::new(|fut| {
                    tokio::spawn(fut);
                }))
                .build()
        };

        Ok(Client { id_keys, swarm })
    }

    pub fn send_message(&mut self, message: &str) -> crate::Result<()> {
        // https://stackoverflow.com/questions/26593387
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_millis()
            .try_into()
            .expect("time overflowed u64");

        let command = Command::Message {
            contents: message.to_owned(),
            timestamp,
        }
        .encode()?;

        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(gossipsub::IdentTopic::new(TOPIC), command)?;

        Ok(())
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

    fn handle_event<OtherErr>(
        &mut self,
        event: SwarmEvent<
            ComposedEvent,
            EitherError<GossipsubHandlerError, OtherErr>,
        >,
    ) -> Option<ClientEvent> {
        match event {
            SwarmEvent::Behaviour(ComposedEvent::Gossipsub(
                GossipsubEvent::Message {
                    propagation_source: source,
                    message_id: _,
                    message,
                },
            )) => {
                // TODO is this the best way to handle decoding issue?

                let packet = Command::decode(&message.data);

                if let Err(err) = &packet {
                    warn!("Could not decode message: {:?}", err);
                }

                match packet.unwrap() {
                    Command::Message {
                        contents,
                        timestamp: _,
                    } => {
                        return Some(ClientEvent::Message { contents, source });
                    }
                }
            }
            SwarmEvent::Behaviour(ComposedEvent::Mdns(event)) => match event {
                MdnsEvent::Discovered(list) => {
                    for (peer, _) in list {
                        self.swarm
                            .behaviour_mut()
                            .gossipsub
                            .add_explicit_peer(&peer);
                    }
                }
                MdnsEvent::Expired(list) => {
                    for (peer, _) in list {
                        let behaviour = self.swarm.behaviour_mut();
                        if !behaviour.mdns.has_node(&peer) {
                            behaviour.gossipsub.remove_explicit_peer(&peer);
                        }
                    }
                }
            },
            _ => {}
        }

        None
    }
}

impl Stream for Client {
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

fn message_id_fn(
    message: &gossipsub::GossipsubMessage,
) -> gossipsub::MessageId {
    let mut hasher = DefaultHasher::new();
    message.source.hash(&mut hasher);
    message.data.hash(&mut hasher);
    gossipsub::MessageId::from(hasher.finish().to_string())
}

// TODO seeds

pub fn gen_id_keys() -> Keypair {
    Keypair::generate_ed25519()
}

pub fn gen_static_keypair(
    id_keys: &Keypair,
) -> crate::Result<AuthenticKeypair<X25519Spec>> {
    Ok(noise::Keypair::<noise::X25519Spec>::new().into_authentic(id_keys)?)
}

const NUM_NAMES: usize = 4;
const NAMES: [&str; NUM_NAMES] = ["alice", "bailie", "charlotte", "danielle"];

pub fn name_from_peer(peer_id: PeerId) -> &'static str {
    // obviously not very smart or "secure"
    let sum = peer_id
        .to_bytes()
        .iter()
        .fold(0u8, |acc, b| acc.wrapping_add(*b));
    NAMES[sum as usize % NUM_NAMES]
}
