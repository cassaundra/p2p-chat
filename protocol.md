# Peer-to-Peer chat

**Note:** this specification is not finalized and is subject to change without warning.

## Introduction

`p2p-chat` is a peer-to-peer chat service inspired by IRC.

The following protocols are referenced within this document:

- libp2p addresing
  (<https://github.com/libp2p/specs/blob/master/addressing/README.md>)
- libp2p connection establishment
  (<https://github.com/libp2p/specs/blob/master/connections/README.md>)
- libp2p peer IDs and keys
  (<https://github.com/libp2p/specs/blob/master/peer-ids/peer-ids.md>)
- libp2p Gossipsub v1.1
  (<https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.1.md>)
- libp2p Kademlia DHT specification
  (<https://github.com/libp2p/specs/blob/master/kad-dht/README.md>)
- Noise Protocol
  (<http://noiseprotocol.org/noise.html>)
- msgpack
  (<https://github.com/msgpack/msgpack/blob/master/spec.md>)

## Definitions

### Channel

A **channel** is defined by:

- A human-readable identifier, consisting of UTF-8 characters.
- A version number, represented by a non-negative integer.
- A channel owner.
- A list of participating peers.

Channels are referenced by their owner (as a namespace) and their
identifier.

### Peer

A **peer** is defined by a keypair and multihash.

## Communication

Peers communicate over libp2p gossipsub, with topics corresponding to the identifier of each channel.
Nickname updates are sent over the "nick" topic.

### Discovery

Peers can discover one another in the following ways:

- By using mDNS over their network.
- By dialing known multi-address.

### Connection establishment

Connections are established in accordance with the protocol negotiation procedure defined in V1 multistream-select protocol.
The handshake is performed using a Noise `XX` handshake pattern.

### Encoding

Messages are encoding with msgpack, with indexed fixmaps to denote
variants

### Message types

All messages have timestamps and are signed as part of libp2p pub/sub.

#### Channel create

A *channel create* message contains:

- The identifier of the channel being created.
- The owner of the channel.
- A non-empty list of participating peers.

The version number is presumed to be zero at channel creation.

#### Channel upgrade

A *channel update* message contains:

- The identifier of the channel being upgraded.
- The new version number (which must be greater than the previous version number).
- The channel owner (may differ from the current channel owner).
- A non-empty list of participating peers.

#### Channel request join

A *channel request join* message contains:

- The identifier of the channel the user wishes to join.

#### Channel request leave

A *channel request leave* message contains:

- The identifier of the channel the user wishes to leave.

#### Message send

A *message send* command contains:

- UTF-8 encoded message (no more than 4096 bytes).
- The type of message, encoded as an indexed integer, from the following:
    - Normal.
    - Me (from a `/me` message).

#### Change nickname

A *change nickname* message contains:

- The user's new nickname.

## Behaviour

### Validation

Peers validate incoming messages before propagating them to the network.
In most cases, peers should reject invalid messages, thereby reducing the peer affinity score for the rejected sender.

### Nicknames

Every peer starts out with no nickname assigned.
Clients may choose to represent these unnamed peers however they would like, such as with a human-readable name derived from the peer's multihash.

A peer may send out a *change nickname* command with a new nickname, which causes the new nick to be inserted into a distributed hash table of peers and their nicknames.

Nickname collisions between peers are permitted.
Clients are left to appropriately disambiguate between these possible collisions.

### Channels

New channels may be created at any time by anyone via the *channel create* announcement.
The announcing peer takes on the role of *channel owner* and may update the channel manifest by posting a *channel upgrade* message.
The details of newly created channels are stored in a distributed hash table.

In the case of a conflict between *channel upgrade* messages, one should be chosen as correct by some yet undecided arbitrary procedure (such as checking if the XOR of the hashes of the two channels is even).

## Future work

The following elements of this specification are left up to future consideration.

### Authority in channel joining and parting

Currently, a channel owner is fully authoritative of the list of participants in their room.
Although users may ignore the messages belonging to the channels they do not wish to participate in, they will nevertheless be included as part of the channel definition.
Instead, an invitation system could be devised that requires all room-joining users to post an "acceptance" or "rejection" message upon invitation to a room.
They may also leave the room at will with a similar mechanism.
Under this system, channel owners would still retain the right to invite or kick users at will.

### Remove channel feature

Currently, no mechanism exists for deleting channels.
This is mostly due to simplify the operation of the distributed hash table, since it can essentially be represented as an immutable data structure.
