<div align="center">

# link2linux

**Your Android phone, right on your Linux desktop.**

Notifications, calls, files and your clipboard, shared directly between
your phone and your computer over your own network.
No account. No cloud. Open source.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
![Platforms: Linux | Android](https://img.shields.io/badge/platforms-Linux%20%7C%20Android-informational.svg)
![Status: pre-release](https://img.shields.io/badge/status-pre--release-orange.svg)

[Website](https://codesensei-tushar.github.io/link2linux/) ·
[Protocol](docs/protocol.md) ·
[Architecture](docs/architecture.md) ·
[Contributing](CONTRIBUTING.md)

</div>

---

> **Status:** link2linux is in active development. Version 0.3.0 is out as a
> pre-release, with pairing, notifications, clipboard sharing, file
> transfer, calls, phone status and ring-my-phone. Get it from the
> [releases page](https://github.com/codesensei-tushar/link2linux/releases).

## Why link2linux

Your phone already knows about every message, call and photo. Your desktop
is where you spend the working day. link2linux joins the two, and does it on
your terms.

- **Local-first.** The phone and the computer talk directly over your local
  network. There is no account to create, no relay server and no cloud copy
  of your data.
- **Encrypted end to end.** Every connection uses mutual TLS 1.3. Each device
  creates its own keys, and a device only accepts the one it was paired
  with.
- **Pair once, in seconds.** Scan a QR code on your desktop with your phone.
  The two devices find each other on their own from then on.
- **Built for Linux.** A native desktop app for GNOME, KDE Plasma and other
  desktops, on Wayland and X11, packaged as .deb, .rpm and AppImage.
- **You decide what's shared.** Every feature that shares your content
  starts switched off, and Android asks for each permission only when you
  turn that feature on.
- **Open source.** Apache-2.0, all of it: the desktop app, the Android app and
  the protocol.

## Features

| Feature | What it does | Status |
|---|---|---|
| Pairing | Pair with a QR code, or compare a 6-digit code if you have no camera | Available (v0.1) |
| Notifications | Phone notifications on your desktop: reply inline, texts included, run actions, dismiss on both devices | Available (v0.1) |
| Clipboard | Copy on one device, paste on the other | Available (v0.2) |
| Files | Send files both ways with drag and drop or the share sheet | Available (v0.2) |
| Calls | See who's calling and talk through your computer over Bluetooth | Available (v0.3) |
| Phone status | Battery, signal and ring-my-phone | Available (v0.3) |
| Photos | Browse your phone's recent photos on your computer and save the ones you want | In progress (v0.4) |
| Screen | Mirror your phone in a window on your desktop, and control it from there if you allow it | In progress (v0.4) |

## How it works

```
  Android phone                                   Linux desktop
 ┌─────────────────────────┐                     ┌──────────────────────────┐
 │ link2linux for Android  │   mutual TLS 1.3    │ link2linux desktop app   │
 │                         │◄───────────────────►│                          │
 │  • announces itself on  │   protobuf frames   │  • finds the phone and   │
 │    the local network    │   on your own LAN   │    connects to it        │
 │  • listens for its      │                     │  • shows notifications,  │
 │    paired desktop       │                     │    messages and more     │
 └─────────────────────────┘                     └──────────────────────────┘
```

1. **Pair.** The desktop shows a QR code with its address, its key's
   fingerprint and a one-time secret. The phone scans it, the two connect,
   and each proves it holds the secret before it trusts the other's key.
2. **Discover.** The phone announces itself on your network with a rotating,
   anonymous tag that only its paired desktops can recognise.
3. **Connect.** The desktop dials the phone over TLS, pinned to the key it
   stored at pairing. Messages are compact protobuf frames, defined once
   in [`proto/`](proto/link2linux/v1) and shared by both apps.

The full design, including the threat model, is in the
[protocol specification](docs/protocol.md). Major decisions are recorded as
[architecture decision records](docs/adr).

## Install

Install the desktop app with one command:

```sh
curl -fsSL https://codesensei-tushar.github.io/link2linux/install.sh | sh
```

The script picks the right package for your distribution and checks it
against the release's `SHA256SUMS` before installing. Or download a package
yourself from the [releases page](https://github.com/codesensei-tushar/link2linux/releases):

| Download | For |
|---|---|
| `link2linux_amd64.deb` | Debian, Ubuntu, Linux Mint, Pop!_OS and other derivatives |
| `link2linux.x86_64.rpm` | Fedora, openSUSE and other RPM-based distributions |
| `link2linux-x86_64.AppImage` | Any other x86_64 distribution |
| `link2linux.apk` | Android 8.0 and newer |

ARM64 builds and Flatpak are planned.

**Requirements:** a 64-bit Linux desktop and an Android phone on the same
network, or the computer joined to the phone's own hotspot. Calls need
Bluetooth on the computer.

## Privacy and security

- Nothing leaves your network. There are no servers to send data to, and no
  telemetry.
- Pairing uses a one-time secret from the QR code, bound to the TLS session,
  so a device on your network can't pose as your phone or your desktop.
- Network announcements carry no device names or stable identifiers.
- One-time codes stay on your phone, on every Android version: the desktop
  shows that a code arrived, but not the code. This is on by default and can
  be turned off on the phone. Other content that Android marks as sensitive
  stays hidden too.
- The desktop keeps the key that identifies it to your phone in the system
  keyring (GNOME Keyring, KWallet or KeePassXC), or in a file only you can
  read when there is no keyring.
- Desktop pop-ups only name the app unless you turn on their text in
  Settings, and never show it while the screen is locked. The full
  notification stays in the link2linux window.
- Pictures that come with notifications stay off the disk: in your login
  session's runtime folder, which is cleared when you log out, or only in
  the app's memory.
- Other programs can't attach a debugger to the desktop app or read its
  memory. The window's web view runs in separate processes that this doesn't
  cover.
- Linux doesn't keep apps apart the way Android does. Any program you run
  outside a sandbox can read your files and ask your keyring for its
  secrets. Running apps you don't trust in a sandbox (Flatpak or Snap) is
  what keeps them away from your data.

Found a security issue? Please report it privately as described in
[CONTRIBUTING.md](CONTRIBUTING.md#reporting-security-issues).

## Roadmap

- **v0.1:** QR pairing, automatic reconnection and notifications with reply
  and dismiss sync.
- **v0.2:** clipboard sharing and file transfer.
- **v0.3:** calls over Bluetooth, phone status and ring-my-phone.
- **v0.4:** photos, screen mirroring and control, ARM64 builds and Flatpak.

## Project structure

| Path | What lives there |
|---|---|
| [`apps/desktop`](apps/desktop) | Desktop app: a Tauri v2 shell with a React and TypeScript interface |
| [`apps/android`](apps/android) | Android app: Kotlin and Jetpack Compose, multi-module Gradle build |
| [`crates/core`](crates/core) | Discovery, pairing, secure transport, sessions and feature logic |
| [`crates/protocol`](crates/protocol) | Wire types generated from the protobuf schema |
| [`crates/platform`](crates/platform) | Operating-system integration behind traits, Linux first |
| [`proto/link2linux/v1`](proto/link2linux/v1) | The protobuf schema both apps share |
| [`docs`](docs) | Architecture, protocol specification and decision records |
| [`website`](website) | The project website |

## Contributing

Contributions are welcome, from bug reports and testing on your
distribution or phone to code and documentation.
[CONTRIBUTING.md](CONTRIBUTING.md) explains how to build and run both apps
locally, the checks a change must pass, and how to propose protocol changes.

## License

link2linux is licensed under the [Apache License 2.0](LICENSE).
