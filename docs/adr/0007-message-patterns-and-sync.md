# 7. Message patterns and the sync model

Status: accepted

## Context
Features exchange very different kinds of traffic over one session: state
that lives on one device and is mirrored on the other (notifications,
messages, contacts), actions the user takes on one device that must happen
on the other (reply, dismiss), large blobs (icons, photos), and liveness
checks. Connections drop often, because phones sleep and change networks,
and the two apps ship independently (an app store on one side, distribution
packages on the other), so either can be older than the other.

## Decision
- **One Envelope for everything.** Each frame carries an Envelope with a
  per-sender `seq` and one typed payload from a `oneof`, with a block of
  field numbers per feature.
- **Requests and replies by sequence number.** A request sets
  `expects_reply`; its answers carry `reply_to` with the request's `seq`.
  `Ack` and `ErrorReply` are the shared answers; requests that return data
  name their own reply message. A receiver that does not know a request
  answers `ERROR_CODE_UNSUPPORTED`, so a sender never waits on a message
  the peer cannot read.
- **One-way events** for live changes and liveness, without answers.
- **Capability tokens.** Hello lists the feature names, and optional parts
  written `feature.option`, that the build implements. A feature is active
  when both sides list it. `FeatureStatus` reports at runtime whether each
  feature is ready, turned off or waiting for a permission. Incompatible
  changes get a new token instead of a version number.
- **Delta sync with cursors.** The device that owns a collection numbers
  its changes with revisions within an epoch. The other device keeps a
  cursor (epoch, revision) and asks for the changes since it; the owner
  answers with a delta, or with a snapshot when it cannot. The request also
  subscribes to live events, which carry their revisions.
- **A durable outbox for user actions.** Actions are stored before they are
  sent, retried after reconnects with the same idempotency key, and removed
  when an answer arrives. Receivers remember keys for 24 hours and carry out
  each action once.
- **Content-addressed images.** Messages carry an image's SHA-256; the
  receiver fetches what it lacks and caches it by hash.

Rejected alternatives:
- **String routes with generic key-value payloads.** No compile-time
  checks, and the two implementations drift apart. Typed messages from one
  schema avoid both.
- **Transport-level acknowledgements for every message.** TCP already
  delivers in order within a connection. What actually needs confirming is
  the outcome of an action (replies and the outbox) and state lost while
  disconnected (cursors).
- **The follower sends its inventory (ids and checksums) for the owner to
  diff.** Every reconnect would cost a transfer proportional to the
  collection's size. The owner can work out what changed locally, so a
  reconnect after a short gap costs one small round trip.
- **gRPC or another RPC framework over HTTP/2.** A large dependency on
  Android for a long-lived, two-way link between peers, with client and
  server roles that do not match who dials whom. Our framing is a few dozen
  lines on each side.
- **Version numbers per feature.** They multiply into a matrix of
  combinations to support; tokens combine freely.
- **Images inline in each message.** Icons would be sent again with every
  notification.

## Consequences
Every feature follows the same recipe: messages in its field-number block,
a capability token, and, for mirrored state, a collection with a cursor.
The owner of a collection must keep a change log with tombstones, or accept
that followers get snapshots after it loses the log. Followers must store
their cursor in the same transaction as the data it describes. Receivers
must keep idempotency keys, and senders must handle timeouts and late
answers. The rules are written once, in `docs/protocol.md`, and apply to
every feature.
