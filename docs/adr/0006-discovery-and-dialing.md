# 6. Discovery, and the desktop dials the phone

Status: accepted

## Context
On Linux desktops, host firewalls (ufw, firewalld) commonly drop inbound
connections while allowing outbound ones and inbound mDNS multicast. Asking
users to open ports is a support burden and a security trade-off they
should not have to make.

The Android app can accept TCP connections while its connected-device
foreground service runs. From target API 37, Android gates every local
network use behind one permission, in both directions, so listening on the
phone costs nothing extra. Receiving is another matter: while the app is in
the background, the Wi-Fi firmware drops multicast and broadcast frames
unless the app holds a multicast lock, which keeps the phone awake. A
background phone therefore cannot be asked where it is; it has to say so
itself.

Some networks drop multicast but pass broadcast or unicast; networks with
client isolation pass nothing between devices. A phone also moves between
networks, and anything it announces can be seen by everyone on each of
them. Android's companion-device presence, which lets the system start the
app when a paired device comes near, works only with Bluetooth or Bluetooth
LE devices, not with hosts on a Wi-Fi network.

## Decision
- **The phone listens; the desktop dials.** The phone accepts TLS
  connections on TCP port 27380. The desktop never accepts TCP connections;
  it is always the TCP and TLS client.
- **Discovery flows from the phone to the desktop.** The desktop never
  depends on an answer to a query; it listens for what the phone sends.
- **Last known address first.** On start, resume, network change and
  connection loss, the desktop first re-dials the address of the last
  session, which is usually still right.
- **mDNS.** The phone announces `_link2linux._tcp` with a random instance
  name and a TXT record without stable identifiers. A paired desktop
  recognizes it by a tag that rotates with a random nonce, computed with a
  key the phone gave it at pairing. During pairing, a tag derived from the
  QR secret marks the phone that scanned the code. The phone re-registers
  the service when it wants to be found, which sends fresh announcements.
- **Beacons on UDP port 27381.** For networks that filter multicast, the
  phone sends small, fixed-layout UDP beacons, by broadcast and by unicast
  to the desktop addresses it knows, in bursts when it wants to be found.
  Each carries a timestamp, a nonce and a tag keyed like the mDNS tag, so
  the desktop recognizes its phone and drops replays.
- **A typed address** is the last resort.
- **Only hints.** Discovery results only trigger dials, and the desktop
  dials only on-link addresses. Trust comes from TLS alone.
- **Bluetooth LE for presence only.** The desktop may advertise a fixed
  service UUID, connectable and from its stable address, so the phone can
  make a plain companion-device association with exactly that desktop (the
  QR code carries its Bluetooth address). No data flows over Bluetooth in
  version 1.

Rejected alternatives:
- **The phone dials the desktop.** It fails on every desktop whose
  firewall drops inbound connections, unless the user opens a port.
- **Both sides listen and dial.** Two directions mean duplicate connections
  to reconcile, and they add little reach: whenever an inbound connection to
  the desktop would work, a beacon reaches it too, and the desktop then
  dials.
- **The desktop probes and the phone answers.** A background phone does not
  receive the probe, and a desktop firewall drops a unicast answer to a
  broadcast, because it cannot match it to the probe.
- **Ports in the ephemeral range.** A fixed port there can collide with the
  source ports of outgoing connections, so both ports sit below 32768.
- **The system's Avahi daemon over D-Bus.** It is not installed or running
  everywhere, and no maintained Rust binding exists. Our own mDNS stack
  works wherever a UDP socket does.
- **A stable device id in the announcement.** It would let anyone on any
  network the phone joins recognize it again.
- **Bluetooth LE as a data or bootstrap channel.** Slower, more
  permissions and more code, for no gain on a shared network.
- **The desktop announcing itself too.** Pairing starts on the desktop, so
  the phone never needs to find one; staying silent keeps the desktop out
  of other people's network scans.

## Consequences
Pairing and reconnecting work with a default desktop firewall on any
network that passes multicast. Where multicast is filtered, they work when
the desktop allows UDP port 27381, so packages ship firewalld and ufw
service definitions for it. Networks with client isolation cannot work at
all, and the apps say so. The phone has to run a foreground service while
it is paired, which it needs anyway to stay connected, and spends a little
battery on beacon bursts while it is not connected. The phone's
announcements reveal that a link2linux phone is present, but not which one.
