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

## `v1/pairing.txt`

Inputs and expected outputs for every hash, tag and code that
`docs/protocol.md` defines on top of TLS: key fingerprints, the exporter
context, the mDNS presence and pairing tags (also in their TXT form), a
complete beacon, a QR code URI, both QR pairing proofs, and the SAS
commitment and code. The exporter values themselves are inputs, because
TLS derives them; interoperability tests between the two TLS stacks cover
that part.

Each line is `name = value`, with byte strings in lowercase hex. A
conforming implementation computes every output from the inputs.
`pairing.sh` rebuilds the file with `openssl` and `xxd`.

## Who uses them

- Rust: `crates/protocol/tests/codec.rs` reads `frames.bin`.
- Android: `FrameCodecTest` in `apps/android/core/network` reads the same file.
- The pairing implementations on both sides check `pairing.txt`.
