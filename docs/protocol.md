# Protocol (draft)

This is a working draft. The message set will grow feature by feature, and
every change lands in `proto/link2linux/v1` first.

## Layers

1. **Discovery**: how devices find each other on the local network.
2. **Pairing**: a one-time trust step. Both screens show the same short
   code, and the user confirms it on both. Each side stores the other's
   certificate fingerprint.
3. **Transport**: TLS with the pinned certificates from pairing.
4. **Framing**: each frame is a varint length prefix followed by one
   serialized `Envelope`.
5. **Session**: both sides send `Hello` first (protocol version, device
   identity, supported features), then exchange feature messages.

## Versioning

- The package name carries the major version (`link2linux.v1`).
- Within a major version, changes must be backward compatible: add fields
  and messages, and never reuse or renumber field tags.
- `Hello.protocol_version` lets peers refuse or degrade when majors differ.

## Messages

| Message | Direction | Purpose |
|---|---|---|
| `Hello` | both | Identity, version, features |
| `Ping` / `Pong` | both | Liveness, round-trip time |

More messages are added as each feature is designed.
