# 5. Local-only pairing and trust

Status: accepted

## Context
The phone and the desktop must come to trust each other with no account and
no server, on networks that other people share. Pairing has to resist an
attacker on the local network who can read, drop and inject packets, and it
has to stay simple: one action from the user in the common case.

Two stacks have to implement it. The desktop uses rustls, which supports
custom certificate checks and the TLS keying material exporter. The Android
app supports API 26 and later, where the platform has TLS 1.3 only from API
29 and the exporter only from API 31, and it keeps its key in the Android
Keystore, so it cannot use TLS extensions or handshakes that its TLS stack
does not expose. Almost every phone has a camera; a few users
still cannot scan a code.

## Decision
- **Identity.** Each installation has a device id and an ECDSA P-256 key
  with a self-signed certificate. Devices recognize each other only by the
  SHA-256 of the key's SubjectPublicKeyInfo (`fp`). Names, dates and
  extensions in the certificate are ignored, so certificates never expire
  in a way that matters.
- **Transport trust.** Every connection is mutually authenticated TLS 1.3.
  Each side checks the CertificateVerify signature and then the peer's
  `fp` against its pairing records. No session resumption, no 0-RTT.
- **QR code pairing (default).** The desktop shows a QR code with its `fp`,
  a 128-bit one-time secret and its addresses. The phone pins the desktop's
  key from the code. Inside the TLS connection, each side sends an
  HMAC-SHA256 of the secret over its role and a TLS exporter value whose
  context holds both fingerprints. The exporter binds the proof to this
  connection, and the context binds it to both keys, including the
  client's, which the TLS 1.3 exporter alone does not cover.
- **SAS pairing (fallback).** Both screens show a six-digit code computed
  from an exporter value (with its own label, and the same context) and
  two nonces. The phone commits to its nonce before it sees the desktop's,
  and reveals it after, so a man in the middle has one blind guess per
  attempt.
- **Order of storing.** The desktop stores the pairing first, after the
  phone has accepted; the phone stores it when the desktop's acceptance
  arrives. An interrupted pairing is detected on the next dial.
- **One TLS stack on every phone.** The Android app bundles Conscrypt and
  uses it for its own connections on every API level, so every phone has
  TLS 1.3 and the exporter. A phone that cannot export keying material does
  not pair.
- **One key type.** ECDSA P-256 with SHA-256 on both sides, the type both
  TLS stacks sign and verify everywhere.
- **Lifecycle.** Re-pairing replaces a record. Unpairing sends a Goodbye
  when a session is up, and otherwise shows up as a rejected certificate.
  Key rotation announces the next key's `fp` over an authenticated session
  and switches on the next connection.

Rejected alternatives:
- **A PAKE (SPAKE2, CPace) over a typed code.** Sound, but it needs a
  handshake of our own outside the platform TLS on Android, and a second
  cryptographic implementation on both sides. The QR code carries far more
  entropy than anyone would type.
- **Noise instead of TLS.** No platform implementation on Android, so we
  would ship and maintain one. TLS 1.3 has mature, reviewed stacks on both
  sides.
- **A short code from the fingerprints or the exporter alone.** An active
  attacker can generate keys until the two codes it causes collide; for six
  digits that takes seconds. The commitment is what makes six digits safe.
- **Trust on first use without any check.** Gives no protection against an
  attacker present during pairing, which is exactly when one would act.
- **TLS with a pre-shared key from the QR code, or raw public keys.**
  Neither is exposed by the Android platform TLS API in a way we can rely
  on. An X.509 wrapper whose fields we ignore costs nothing.
- **Proofs without the exporter for older phones.** They would still hold
  if every certificate check were implemented correctly, but the exporter is
  the defense for the day one is not. Bundling Conscrypt keeps the rule
  uniform.

## Consequences
Pairing needs both devices on the same network, and no third party is
involved at any point. Users scan one code; users without a camera compare
six digits on two screens. A device that loses its private key pairs again.
The Android app carries Conscrypt, about 1.7 MB per ABI, and has to follow
its security releases. Both implementations must agree on the exact bytes
that are hashed, so the spec defines them precisely and both test suites
check the same test vectors.
