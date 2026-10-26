# Changelog

All notable changes to link2linux are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Releases before 1.0
are published as pre-releases. `docs/releasing.md` says how a section moves
from Unreleased to a version.

## [Unreleased]

### Fixed

- The phone keeps listening, announcing and sending beacons while it hosts a
  Wi-Fi hotspot, or USB or Bluetooth tethering, and has no other network, so a
  computer that joins the phone's hotspot finds it.

## [0.1.0] - 2026-10-26

The first release: an Android phone and a Linux desktop pair once, then find
each other on the local network by themselves, and the phone's notifications
appear on the desktop.

### Added

- **Pairing.** Pair a phone by scanning the QR code the desktop shows. A phone
  that can't scan pairs by comparing a six-digit code on both screens instead.
- **Finding each other.** Once paired, the desktop finds the phone on the local
  network through mDNS, or through the phone's own announcements where the
  network filters multicast, and reconnects by itself after the phone sleeps
  or changes networks.
- **Notifications.** The phone's notifications appear on the desktop with their
  action buttons. Reply to messages from the desktop: inside the notification
  on KDE Plasma, and from the link2linux window elsewhere. Dismissing a
  notification on the desktop dismisses it on the phone too; this dismiss sync
  is on by default and can be turned off. Content that Android withholds from
  other apps, such as one-time codes, shows as hidden rather than as
  placeholder text.
- **Desktop app.** Pages for pairing and notifications, and the connection
  status of each paired phone.
- **Phone app.** For Android 8.0 and newer. Each feature asks for its Android
  permission only when you turn it on.
- **Downloads.** A .deb, an .rpm and an AppImage for 64-bit x86 Linux, and a
  signed APK, each with a SHA-256 checksum and a build provenance attestation,
  plus a one-line installer for the desktop app.

### Security

- Everything stays on the local network: no account, no server and no relay.
- Every connection is mutually authenticated TLS 1.3, with each device's own
  P-256 key pinned at pairing. The QR code's one-time secret and the six-digit
  code are bound to the TLS connection, so they can't be relayed to another
  device.
- The desktop never accepts incoming connections: it dials the phone, and only
  addresses on its own local networks. The firewalld and ufw definitions that
  come with the packages open only mDNS and the beacon port, and only once you
  allow them.

### Known limitations

- Messages, calls, photos, files, the clipboard and screen mirroring are not
  in this release.
- GNOME has no reply field inside notifications, so replies go through the
  link2linux window there.
- There are no ARM64 builds and no Flatpak yet.

[Unreleased]: https://github.com/codesensei-tushar/link2linux/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/codesensei-tushar/link2linux/releases/tag/v0.1.0
