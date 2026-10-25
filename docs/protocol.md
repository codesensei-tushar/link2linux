# link2linux protocol, version 1

This document specifies how the link2linux desktop app and the link2linux
Android app find each other on a local network, establish trust, and
exchange messages. The wire messages are defined once, in
[`proto/link2linux/v1`](../proto/link2linux/v1); this document gives the
rules around them. The reasons behind the main choices are in the decision
records: [pairing and trust](adr/0005-local-pairing-and-trust.md),
[discovery and dialing](adr/0006-discovery-and-dialing.md) and
[message patterns and sync](adr/0007-message-patterns-and-sync.md).

Everything stays on the local network. There is no account, no server and
no relay: the two devices talk to each other directly or not at all.

## Contents

- [Conventions](#conventions)
- [Identity](#identity)
- [Discovery](#discovery)

## Conventions

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in
BCP 14 (RFC 2119, RFC 8174) when they appear in capitals.

- **Roles.** Every connection is between one *desktop* and one *phone*.
  The desktop always opens the TCP connection and is always the TLS
  client. The phone always listens.
- **Notation.** `||` is byte concatenation. Text labels in cryptographic
  inputs are ASCII bytes with no terminator. Integers in cryptographic
  inputs are big-endian.
- **Fingerprints.** `fp(X)` is the SHA-256 hash of device X's public key,
  computed over the DER encoding of the certificate's
  SubjectPublicKeyInfo. It is 32 bytes. `fp_D` is the desktop's, `fp_P`
  the phone's.
- **base64url** is RFC 4648 section 5, without padding.
- **Times** on the wire are Unix time in milliseconds, in `int64` fields
  named `*_unix_ms`.
- **Text** is UTF-8. Lengths and limits are in bytes.
- **Enum values** are written without their type's prefix where the type
  is clear: `READY` stands for `FEATURE_AVAILABILITY_READY`.

### Constants

| Name | Value |
|---|---|
| mDNS service type | `_link2linux._tcp.local.` |
| Phone TCP port (default) | 47474 |
| Discovery UDP port | 47474 |
| Discovery datagram magic | `L2LD` (0x4C 0x32 0x4C 0x44) |
| Bluetooth LE service UUID | `d0135d8c-397d-4afe-b65f-bb4c33308f6d` |
| TLS ALPN protocol ID | `link2linux` |
| TLS exporter label | `EXPORTER-link2linux-pairing-v1` |
| QR code URI | `link2linux://pair?...` |
| Maximum frame body | 4 MiB |
| Maximum image size | 1 MiB |
| Protocol major version | 1 |

## Identity

Each installation of either app has:

- a **device id**: a random UUID, written in lowercase hyphenated form,
  created on first run and kept until the app's data is cleared;
- a **device name**: shown to the other device, at most 64 bytes, chosen by
  the user and defaulting to the host or model name;
- an **identity key**: an ECDSA P-256 key pair, created on first run, and
  a self-signed X.509 certificate for it.

The desktop keeps its private key in the Secret Service keyring when one is
available, and otherwise in a file only the user can read (mode 0600). The
phone keeps its private key in the Android Keystore, where it cannot be
exported.

Peers recognize each other by `fp`, never by anything else in the
certificate: names, validity dates and extensions are ignored. A device can
therefore reissue its certificate for the same key at any time, for example
when it expires, without affecting its peers.

A **pairing record** is what a device stores about a peer it trusts: the
peer's device id, name, kind, current `fp`, an optional next `fp` (see
[key rotation](#key-rotation)), and the time of pairing. A desktop also
stores the phone's presence key and last known addresses; a phone stores
the desktop's Bluetooth LE address and last known addresses.

## Discovery

The phone listens for connections and makes itself findable. The desktop
looks for phones and dials them. The desktop never needs an open inbound
port, so a desktop firewall that blocks inbound connections, which is
common, does not get in the way.

The phone listens on TCP port 47474. If that port is taken it MAY listen on
any other port, which it then advertises. It listens while its connection
service runs, which is whenever it has a pairing (or a pairing is under
way) and is on a Wi-Fi or Ethernet network, unless the user turned
link2linux off.

The desktop learns where a phone is from four sources, all of them hints
that only ever lead to a dial attempt. Trust comes from TLS alone.

1. **mDNS / DNS-SD** announcements from the phone.
2. **UDP datagrams** from the phone: broadcast announcements, and unicast
   *knocks* sent to the desktop's last known addresses.
3. **Last known addresses** from earlier sessions.
4. **An address the user types**, as a last resort.

### mDNS

The phone registers one service of type `_link2linux._tcp` on every
Wi-Fi or Ethernet interface. The SRV record carries its TCP port; the A and
AAAA records carry its addresses. The instance name is `l2l-` followed by 8
random hex digits, picked anew together with the presence nonce below, so
the name says nothing about the phone.

TXT keys (unknown keys MUST be ignored):

| Key | Value | When |
|---|---|---|
| `v` | Highest protocol major version, in decimal | Always |
| `n` | Presence nonce: 8 random bytes, base64url | When the phone has at least one pairing |
| `t` | Presence tag, base64url (see below) | With `n` |
| `q` | Pairing tag of the scanned QR code, base64url | While waiting for the desktop after a QR scan |
| `s` | `1` | While the phone is in SAS pairing mode |
| `d` | The phone's device name | Only in SAS pairing mode |

The **presence tag** lets a paired desktop recognize its phone while
everyone else sees only random values:

```
t = first 8 bytes of HMAC-SHA256(presence_key, "link2linux presence v1" || n)
```

`presence_key` is 32 random bytes that the phone creates once and gives to
each paired desktop in `PresenceUpdate`. The phone picks a fresh nonce, and
with it a fresh instance name, whenever it joins a network and at least
every 15 minutes. A desktop checks a record against each paired phone's key;
the cost is one HMAC per paired phone.

The **pairing tag** lets the desktop that showed a QR code find the phone
that scanned it, without putting the secret on the network:

```
q = first 8 bytes of HMAC-SHA256(secret, "link2linux pairing tag v1")
```

The desktop browses with its own mDNS implementation, so it does not depend
on a system mDNS daemon. On Android, the phone uses the platform's
network service discovery and holds a multicast lock only while it needs to
receive multicast.

### Discovery datagrams

Some networks drop multicast but pass broadcast and unicast. For those, the
phone and the desktop also exchange small UDP datagrams on port 47474:

```
datagram = "L2LD" || kind (1 byte) || entries
entries  = zero or more of: length (1 byte) || "key=value" (length bytes)
```

The entries use the DNS TXT encoding and the same keys as the mDNS TXT
record, plus `port` (the phone's TCP port, in decimal) in announcements. A
datagram is at most 512 bytes. Receivers ignore unknown kinds and keys.

| Kind | Name | Sent by | Sent to |
|---|---|---|---|
| 1 | announce | phone | The broadcast address of each interface, and, as a *knock*, the desktop's known addresses |
| 2 | probe | desktop | The broadcast address of each interface |

- Both sides listen on UDP port 47474.
- The phone broadcasts an announce when it joins a network; every 2
  seconds for 30 seconds after a QR scan and after SAS pairing mode starts;
  and in answer to a probe, at most once per second, by sending its
  announce to UDP port 47474 of the probing address. A reply to the probe's
  source port would not get through: a desktop firewall does not match a
  unicast reply to a broadcast it sent.
- The phone also *knocks*: it sends its announce by unicast to UDP port
  47474 of each address in the QR code (on the same schedule as the
  broadcasts after a scan), and of each paired desktop's last known
  addresses (when it joins a network or its connection service starts). A
  knock reaches a desktop whose firewall allows this one UDP port even
  where broadcast and multicast are filtered.
- The desktop sends bursts of 5 probes, 2 seconds apart, when it starts,
  when its network changes, when a session is lost, when the pairing screen
  opens and when the user asks to reconnect. It does not probe
  periodically.

### Choosing what to dial

The desktop dials a phone when a record or datagram shows:

- a presence tag that matches a paired phone with no live session;
- the pairing tag of the QR code the desktop is showing; or
- SAS pairing mode, for a phone the user picked from the list.

It dials only addresses that are on-link for one of its own interfaces: in
an IPv4 subnet of one of its interfaces, IPv6 link-local, or in an IPv6
/64 prefix of one of its interfaces. It dials an IPv6 link-local address on
the interface where it learned it. This keeps a forged announcement from
steering the desktop towards arbitrary hosts. It also dials the last known
addresses of a paired phone when it starts, when its network changes and on
the reconnect schedule (see [Transport](#transport)). An address the user
types is dialed as given.

### Bluetooth LE

The desktop MAY advertise the link2linux service UUID over Bluetooth LE.
No data flows over Bluetooth in version 1. The advertisement exists so that
the phone can create a companion-device association with the desktop and
ask Android to watch for its presence. Android then starts the phone app
when the desktop comes near, and exempts it from some background limits
while the desktop is present, which keeps the connection service alive
without the phone polling.

- Advertising data: the 128-bit service UUID. Scan response: the desktop's
  device name, truncated to fit, so the system's association dialog can
  show it.
- The desktop advertises from a stable address and tells the phone that
  address in `PresenceUpdate.ble_address`. The phone uses it as the filter
  of its association request, so the dialog offers exactly that desktop.
- The phone treats a presence event only as a reason to start listening
  and announcing. It never treats one as proof of anything.

Bluetooth is optional on both sides. Without it, the phone relies on its
foreground connection service alone.

### Android local network access

Apps that target Android API 37 or later need the `ACCESS_LOCAL_NETWORK`
runtime permission for everything in this section: listening for TCP,
mDNS, and UDP broadcast and unicast. The version 1 phone app targets API 36
and does not need it. The app that targets API 37 asks for it during
onboarding, next to the other nearby-device permissions.
