use std::{
    collections::{hash_map::DefaultHasher, HashMap},
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
        GossipsubMessage, MessageId,
    },
    identity::Keypair,
    kad::{
        record::Key, store::MemoryStore, Kademlia, KademliaEvent, QueryResult,
        Quorum, Record,
    },
    mdns::{self, Mdns, MdnsEvent},
    mplex,
    noise::{self, AuthenticKeypair, X25519Spec},
    swarm::{SwarmBuilder, SwarmEvent},
    tcp::TokioTcpConfig,
    Multiaddr, NetworkBehaviour, PeerId, Swarm, Transport,
};
use log::{info, warn};

use crate::protocol::{
    topic_from_channel, ChannelIdentifier, Command, MemoryKey, MemoryValue,
    MessageType, DEFAULT_GOSSIPSUB_TOPIC,
};

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "ComposedEvent")]
struct ComposedBehaviour {
    gossipsub: Gossipsub,
    kademlia: Kademlia<MemoryStore>,
    mdns: Mdns,
}

#[derive(Debug)]
enum ComposedEvent {
    Gossipsub(GossipsubEvent),
    Kademlia(KademliaEvent),
    Mdns(MdnsEvent),
}

impl From<GossipsubEvent> for ComposedEvent {
    fn from(val: GossipsubEvent) -> Self {
        ComposedEvent::Gossipsub(val)
    }
}

impl From<KademliaEvent> for ComposedEvent {
    fn from(val: KademliaEvent) -> Self {
        ComposedEvent::Kademlia(val)
    }
}

impl From<MdnsEvent> for ComposedEvent {
    fn from(val: MdnsEvent) -> Self {
        ComposedEvent::Mdns(val)
    }
}

/// An event emitted by a [`Client`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ClientEvent {
    Message {
        contents: String,
        channel: ChannelIdentifier,
        timestamp: u64,
        message_type: MessageType,
        source: PeerId,
    },
    UpdatedNickname {
        nick: String,
        source: PeerId,
    },
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    Dialing(PeerId),
    OutgoingConnectionError {
        peer_id: Option<PeerId>,
        error: libp2p::swarm::DialError,
    },
}

/// The client interface to p2p-chat.
pub struct Client {
    nick: String,
    nick_cache: HashMap<PeerId, Option<String>>,
    channels: Vec<ChannelIdentifier>,
    id_keys: Keypair,
    swarm: Swarm<ComposedBehaviour>,
}

impl Client {
    pub async fn new(nick: &str, id_keys: Keypair) -> crate::Result<Self> {
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

        let swarm = {
            //  gossipsub

            let gossipsub = {
                let gossipsub_config =
                    gossipsub::GossipsubConfigBuilder::default()
                        .heartbeat_interval(Duration::from_secs(15))
                        .validation_mode(gossipsub::ValidationMode::Strict)
                        .validate_messages()
                        .message_id_fn(message_id_fn)
                        .build()
                        .unwrap();
                Gossipsub::new(
                    gossipsub::MessageAuthenticity::Signed(id_keys.clone()),
                    gossipsub_config,
                )
                .unwrap()
            };

            let kademlia = {
                let memory_store = MemoryStore::new(peer_id);
                let mut kademlia = Kademlia::new(peer_id, memory_store);

                // we are the provider of our own nick

                let nick_key =
                    Key::new(&MemoryKey::Nickname(peer_id).encode()?);
                let nick_value = MemoryValue::Nickname {
                    user: peer_id,
                    nickname: nick.to_owned(),
                }
                .encode_signed(&id_keys)?;

                kademlia.start_providing(nick_key.clone())?;
                kademlia.put_record(
                    Record::new(nick_key, nick_value),
                    Quorum::One,
                )?;

                kademlia
            };

            let mdns = Mdns::new(mdns::MdnsConfig::default()).await?;

            let mut behaviour = ComposedBehaviour {
                gossipsub,
                kademlia,
                mdns,
            };

            // subscribe to the default topic for network updates
            let topic = gossipsub::IdentTopic::new(DEFAULT_GOSSIPSUB_TOPIC);
            behaviour.gossipsub.subscribe(&topic)?;

            SwarmBuilder::new(transport, behaviour, peer_id)
                .executor(Box::new(|fut| {
                    tokio::spawn(fut);
                }))
                .build()
        };

        let mut nick_cache = HashMap::new();
        nick_cache.insert(peer_id, Some(nick.to_owned()));

        Ok(Client {
            nick: nick.to_owned(),
            nick_cache,
            channels: Vec::new(),
            id_keys,
            swarm,
        })
    }

    /// Join a channel.
    ///
    /// If you are already in this channel, this is a no-op.
    pub fn join_channel(
        &mut self,
        ident: ChannelIdentifier,
    ) -> crate::Result<()> {
        if self.channels.contains(&ident) {
            return Ok(());
        }

        self.swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic_from_channel(&ident))?;
        self.channels.push(ident);

        Ok(())
    }

    /// Leave a channel.
    ///
    /// If you are not in this channel, this is a no-op.
    pub fn leave_channel(
        &mut self,
        ident: ChannelIdentifier,
    ) -> crate::Result<()> {
        if let Some(idx) = self.channels.iter().position(|c| *c == ident) {
            self.channels.remove(idx);
            self.swarm
                .behaviour_mut()
                .gossipsub
                .subscribe(&topic_from_channel(&ident))?;
        }

        Ok(())
    }

    /// Post a message to a given channel.
    pub fn send_message(
        &mut self,
        message: &str,
        message_type: MessageType,
        channel: ChannelIdentifier,
    ) -> crate::Result<()> {
        // TODO validate locally

        // https://stackoverflow.com/questions/26593387
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_millis()
            .try_into()
            .expect("time overflowed u64");

        let topic = topic_from_channel(&channel);
        let command = Command::MessageSend {
            contents: message.to_owned(),
            channel,
            timestamp,
            message_type,
        };

        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, command.encode()?)?;

        Ok(())
    }

    /// Dial another client.
    ///
    /// This is a non-blocking operation which may cause the following events to be emitted:
    /// - [`ClientEvent::Dialing`]
    /// - [`ClientEvent::PeerConnected`]
    /// - [`ClientEvent::OutgoingConnectionError`]
    pub fn dial(&mut self, addr: Multiaddr) -> crate::Result<()> {
        info!("Dialing {}", addr);
        self.swarm.dial(addr)?;
        Ok(())
    }

    /// Listen on a given address.
    pub fn listen_on(&mut self, addr: Multiaddr) -> crate::Result<()> {
        self.swarm.listen_on(addr)?;
        Ok(())
    }

    /// Get whether or not this client is connected to a given peer.
    pub fn is_connected(&self, peer_id: &PeerId) -> bool {
        self.swarm.is_connected(peer_id)
    }

    /// Get this client's peer ID.
    pub fn peer_id(&self) -> PeerId {
        PeerId::from(self.id_keys.public())
    }

    /// Get this client's nickname.
    pub fn nick(&self) -> &String {
        &self.nick
    }

    pub fn fetch_nickname(
        &mut self,
        peer: &PeerId,
    ) -> crate::Result<&Option<String>> {
        // this seems to be the only way to satisfy the borrow checker ._.
        if self.nick_cache.contains_key(peer) {
            return Ok(self.nick_cache.get(peer).unwrap());
        }

        let key = Key::new(&MemoryKey::Nickname(*peer).encode()?);
        self.swarm
            .behaviour_mut()
            .kademlia
            .get_record(key, Quorum::One);
        self.nick_cache.insert(*peer, None);

        Ok(&None)
    }

    /// Get the list of channels which we are connected to.
    pub fn channels(&self) -> &Vec<ChannelIdentifier> {
        &self.channels
    }

    fn handle_event<OtherErr>(
        &mut self,
        event: SwarmEvent<
            ComposedEvent,
            EitherError<
                EitherError<GossipsubHandlerError, std::io::Error>,
                OtherErr,
            >,
        >,
    ) -> crate::Result<Option<ClientEvent>> {
        match event {
            SwarmEvent::Behaviour(ComposedEvent::Gossipsub(
                GossipsubEvent::Message {
                    propagation_source: source,
                    message_id,
                    message,
                },
            )) => return Ok(self.handle_message(message, message_id, source)),
            SwarmEvent::Behaviour(ComposedEvent::Kademlia(
                KademliaEvent::OutboundQueryCompleted { result, .. },
            )) => {
                match result {
                    QueryResult::GetRecord(Ok(get_record_ok)) => {
                        for peer_record in get_record_ok.records {
                            let record = peer_record.record;
                            let key = MemoryKey::decode(&record.key.to_vec())?;
                            let value = MemoryValue::decode(&record.value)?;

                            match (key, value) {
                                (
                                    MemoryKey::Nickname(key),
                                    MemoryValue::Nickname { user, nickname },
                                ) => {
                                    if user != key {
                                        warn!("Possible key/value mismatch in DHT!");
                                        return Ok(None);
                                    }

                                    self.nick_cache.insert(key, Some(nickname));
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {} // TODO log others
                }
            }
            SwarmEvent::Behaviour(ComposedEvent::Mdns(event)) => match event {
                MdnsEvent::Discovered(list) => {
                    for (peer, multiaddr) in list {
                        let behaviour = self.swarm.behaviour_mut();
                        behaviour.gossipsub.add_explicit_peer(&peer);
                        behaviour.kademlia.add_address(&peer, multiaddr);
                    }
                }
                MdnsEvent::Expired(list) => {
                    for (peer, multiaddr) in list {
                        let behaviour = self.swarm.behaviour_mut();
                        if !behaviour.mdns.has_node(&peer) {
                            behaviour.gossipsub.remove_explicit_peer(&peer);
                        }
                        behaviour.kademlia.remove_address(&peer, &multiaddr);
                    }
                }
            },
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                self.swarm.behaviour_mut().kademlia.add_address(
                    &peer_id,
                    endpoint.get_remote_address().clone(),
                );
                return Ok(Some(ClientEvent::PeerConnected(peer_id)));
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                return Ok(Some(ClientEvent::PeerDisconnected(peer_id)));
            }
            SwarmEvent::Dialing(peer_id) => {
                return Ok(Some(ClientEvent::Dialing(peer_id)));
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error } => {
                return Ok(Some(ClientEvent::OutgoingConnectionError {
                    peer_id,
                    error,
                }));
            }
            _ => {}
        };

        Ok(None)
    }

    fn handle_message(
        &mut self,
        message: GossipsubMessage,
        message_id: MessageId,
        source: PeerId,
    ) -> Option<ClientEvent> {
        let acceptance;

        let evt = match Command::decode(&message.data) {
            Ok(cmd) => {
                if cmd.is_valid() {
                    acceptance = gossipsub::MessageAcceptance::Accept;
                } else {
                    warn!("Rejecting invalid message from {source}");
                    acceptance = gossipsub::MessageAcceptance::Reject;
                }

                match cmd {
                    Command::MessageSend {
                        contents,
                        channel,
                        timestamp,
                        message_type,
                    } => Some(ClientEvent::Message {
                        contents,
                        channel,
                        timestamp,
                        message_type,
                        source,
                    }),
                    Command::NicknameUpdate { nick } => {
                        self.nick_cache.insert(source, Some(nick.clone()));
                        Some(ClientEvent::UpdatedNickname { nick, source })
                    }
                    _ => None,
                }
            }
            Err(err) => {
                warn!("Could not decode message, rejecting: {:x?}", err);
                acceptance = gossipsub::MessageAcceptance::Reject;
                None
            }
        };

        self.swarm
            .behaviour_mut()
            .gossipsub
            .report_message_validation_result(&message_id, &source, acceptance)
            .expect("could not report message validation");

        evt
    }
}

impl Stream for Client {
    type Item = Option<ClientEvent>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // TODO handle error...
        Pin::new(&mut self.swarm)
            .poll_next(cx)
            .map(|e| e.map(|e| self.handle_event(e).unwrap_or(None)))
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

/// Generate a public/private Ed25519 keypair.
pub fn gen_id_keys() -> Keypair {
    Keypair::generate_ed25519()
}

/// Generate a static keypair for this session.
///
/// This should *not* be re-used between sessions.
pub fn gen_static_keypair(
    id_keys: &Keypair,
) -> crate::Result<AuthenticKeypair<X25519Spec>> {
    Ok(noise::Keypair::<noise::X25519Spec>::new().into_authentic(id_keys)?)
}
