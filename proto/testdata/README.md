# Wire-format test vectors

Fixed bytes that the desktop and Android test suites both check against, so
the two implementations cannot drift apart on framing or encoding.

## `v1/frames.bin`

Three frames, back to back, exactly as they travel on a session stream. Each
frame is a varint length prefix followed by one serialized
`link2linux.v1.Envelope` (the format of `writeDelimitedTo` /
`parseDelimitedFrom` on the JVM and of the `link2linux-protocol` codec).

| # | Source | Envelope |
|---|---|---|
| 1 | `v1/01_hello.txtpb` | `seq 1`, desktop `Hello` with all seven features; 2-byte length prefix |
| 2 | `v1/02_ping.txtpb` | `seq 2`, `Ping { nonce: 1234567890123 }` |
| 3 | `v1/03_pong.txtpb` | `seq 3`, `Pong { nonce: 1234567890123 }` |

A conforming implementation must:

1. decode the file into exactly these three envelopes, with nothing left over;
2. encode the same three envelopes into exactly these bytes.

The `.txtpb` files are the readable source; `generate.sh` rebuilds the binary
from them with `protoc`. Inspect it with `xxd proto/testdata/v1/frames.bin`.

## Who uses them

- Rust: `crates/protocol/tests/codec.rs`
- Android: the `core/protocol` unit tests read the same file.
