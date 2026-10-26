# Changelog

All notable changes to link2linux are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Releases before 1.0
are published as pre-releases. `docs/releasing.md` says how a section moves
from Unreleased to a version.

## [Unreleased]

### Security

Fixes from a security review of the pairing, connection and notification
code. None of them was known to be exploited.

- Pairing: a forged message during a reconnect can no longer make the
  computer think the phone removed it, and the phone's real refusal is now
  recognised. Someone answering a QR code's announcement can no longer end
  it. Pairing by code no longer replaces a phone with the same id without
  asking.
- Connections: both apps check a message's size and repeated fields
  before decoding it, and keep messages small until the other side is
  paired, so one message can no longer make either app use hundreds of
  megabytes. The computer stops reading from a peer that does not read its
  answers, keeps looking for phones after a flood of fake announcements,
  and never dials its own addresses or a public address it last saw a
  phone at.
- Phone: its listener closes off Wi-Fi, Ethernet and hotspot networks, and
  connections that have not yet proven who they are have time and number
  limits. A removed computer is cut off before it is told. One paired
  computer can no longer lock another out with a key change.
- Phone notifications: another app's notification icon can no longer stall
  or crash mirroring, and a computer that stops reading no longer holds up
  the others. One-time-code detection catches letter-only codes and sign-in
  links. Notification keys no longer carry contact details. A new setting,
  "Reply while this phone is locked" (on by default), lets you require an
  unlocked phone for replies and actions from a computer.
- Computer: what the phone sends is held to the protocol's size limits;
  pictures must be a PNG, JPEG or WebP of a sensible size before they reach
  any image decoder, and their files are private to you. Pop-ups go out
  under link2linux's own name. When the app cannot tell whether the screen
  is locked, it treats it as locked, and locking takes the text off pop-ups
  still on screen. The phones' presence keys are kept sealed with a key
  from your keyring, and the pairing secret stays out of debug output.
- Releases: the Android signing key never shares a machine with the build,
  and every workflow action is pinned to a fixed version. `install.sh`
  downloads over HTTPS only.

### Fixed

- The phone app no longer crashes when its camera can't start, or when the
  screen that offers to notice a nearby computer goes away.
- With "Stay connected" off, the phone no longer opens its listener or
  restarts its connection service when a paired computer comes near, and a
  removed computer no longer wakes the app.
- On a fresh install, the phone runs nothing in the background until the
  first pairing starts.

## [0.1.1] - 2026-10-26

Keeps more of the phone's private content to the phone, makes the desktop app
share less with other programs on the computer, and lets the computer pair and
connect over the phone's own hotspot.

### Added

- **One-time codes stay on the phone.** Notifications that look like they hold
  a sign-in, payment or verification code, in English or one of thirteen other
  languages, reach the computer as "One-time code hidden. Check your phone",
  with no text, pictures or actions, on every Android version. They can still
  be dismissed from the computer. "Keep one-time codes on your phone" on the
  phone's Notifications screen turns this off; it is on by default.

### Fixed

- The phone keeps listening, announcing and sending beacons while it hosts a
  Wi-Fi hotspot, or USB or Bluetooth tethering, and has no other network, so a
  computer that joins the phone's hotspot finds it.

### Security

- **The desktop's private key is in the keyring.** The key that identifies the
  desktop to your phones now lives in the system keyring (the Secret Service of
  GNOME Keyring, KWallet or KeePassXC) instead of a file. A key from 0.1.0 moves
  there on the first start, and its file is overwritten and deleted. Without a
  keyring, the key stays in a file only you can read, as before. If the keyring
  that holds the key is locked or not running, the app says so and offers to
  try again, rather than making a new key that your phones wouldn't know. Once
  the key has moved, 0.1.0 no longer finds it: going back to 0.1.0 means
  pairing your phones again.
- **Pop-ups hide the notification's text.** Desktop pop-ups pass through the
  desktop's notification service, where other apps can read them, so a pop-up
  now only names the phone's app and says "New notification". The whole
  notification is in the link2linux window, which a click on the pop-up opens.
  The new Settings page can turn the text back on. Even then, pop-ups never
  show it while the screen is locked, and notifications the phone's app marked
  secret get no pop-up at all on a locked screen.
- **Pictures stay off the disk.** Pictures that come with notifications are
  kept in your login session's runtime folder, which is cleared at logout, or
  only in memory without one. The pictures 0.1.0 saved in its data folder are
  deleted, and a phone's pictures are deleted when you unpair it.
- **The desktop app's memory is closed to other programs.** Other programs you
  run can no longer attach a debugger to the app or read its memory, and it
  writes no core dumps. The window's web view runs in separate processes that
  this doesn't cover.

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

[Unreleased]: https://github.com/codesensei-tushar/link2linux/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/codesensei-tushar/link2linux/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/codesensei-tushar/link2linux/releases/tag/v0.1.0
