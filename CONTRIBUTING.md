# Contributing to link2linux

Thanks for your interest in link2linux! This guide covers how to build and
run both apps on your machine, the checks every change has to pass, and how
changes get reviewed and merged.

There are many ways to help besides code:

- **Try it on your setup** and report what works and what doesn't. Distribution, desktop
  environment, Wayland or X11, and phone model all matter.
- **Report bugs** with steps to reproduce, what you expected, and what happened.
- **Improve the docs**, including this file.
- **Pick up an issue.** Comment on it first, so two people don't build the same thing.

For anything larger than a small fix, please open an issue to discuss the
idea before you start. It saves everyone time.

## Contents

- [Prerequisites](#prerequisites)
- [Getting the code](#getting-the-code)
- [Running the desktop app](#running-the-desktop-app)
- [Running the Android app](#running-the-android-app)
- [Pairing your phone with a development build](#pairing-your-phone-with-a-development-build)
- [Project layout](#project-layout)
- [Checks](#checks)
- [Coding guidelines](#coding-guidelines)
- [Changing the protocol](#changing-the-protocol)
- [Commits and pull requests](#commits-and-pull-requests)
- [Reporting security issues](#reporting-security-issues)
- [License](#license)

## Prerequisites

You need a 64-bit Linux machine. Development happens mainly on Ubuntu, but any
recent distribution works.

**System packages** (for the desktop app's webview, tray icon and protobuf):

```sh
# Debian, Ubuntu and derivatives
sudo apt install build-essential curl wget file pkg-config libssl-dev \
  libwebkit2gtk-4.1-dev libxdo-dev libayatana-appindicator3-dev \
  librsvg2-dev protobuf-compiler

# Fedora
sudo dnf group install c-development
sudo dnf install webkit2gtk4.1-devel openssl-devel libxdo-devel \
  libappindicator-gtk3-devel librsvg2-devel protobuf-compiler
```

**Toolchains:**

| Tool | Version | How to get it |
|---|---|---|
| Rust | pinned in `rust-toolchain.toml` | [rustup](https://rustup.rs); it installs the pinned version on first use |
| Node.js | 24 | your package manager, [nvm](https://github.com/nvm-sh/nvm) or [nodejs.org](https://nodejs.org) |
| pnpm | pinned in `package.json` | `corepack enable`; Node's corepack then uses the pinned version |
| JDK | 17 | e.g. Temurin, or your distribution's `openjdk-17-jdk` |
| Android SDK | platform 36, build-tools 36 | Android Studio, or the command-line tools with `sdkmanager` |
| buf | optional | [buf.build](https://buf.build/docs/installation); needed only for protocol changes |

Point `ANDROID_HOME` at your SDK (for example `export ANDROID_HOME=$HOME/Android/Sdk`).
You only need the JDK and Android SDK if you work on the Android app.

Then check everything is in place:

```sh
make doctor
```

## Getting the code

```sh
git clone https://github.com/codesensei-tushar/link2linux.git
cd link2linux
pnpm install
```

## Running the desktop app

```sh
make dev      # run the desktop app with hot reload
make build    # build release packages into target/release/bundle
```

`make dev` starts the Vite dev server for the interface and the Tauri shell
around it. Changes to the React code reload instantly; changes to Rust code
rebuild and restart the app.

To work on the interface alone in a browser, run
`pnpm --filter desktop dev`. The app then uses a built-in preview backend
instead of the Rust one.

## Running the Android app

Open `apps/android` in Android Studio, or use the command line:

```sh
cd apps/android
./gradlew installDebug        # build and install on a connected phone
./gradlew testDebugUnitTest   # unit tests
```

To install on your phone, turn on **Developer options → USB debugging**
(or wireless debugging) and connect it. Use a real phone if you can.
Emulators usually can't see the devices on your local network, so discovery
and pairing won't work from one.

## Pairing your phone with a development build

1. Put the phone and the computer on the same network.
2. Start the desktop app with `make dev`, open **Pairing** and choose
   **Pair a phone**.
3. In the Android app, scan the QR code.

If the two can't find each other, check your firewall. The desktop app needs
to receive mDNS (UDP 5353) and the phone's beacons (UDP 27381):

```sh
# firewalld (Fedora and others)
sudo firewall-cmd --add-service=mdns --add-port=27381/udp

# ufw (Ubuntu and others)
sudo ufw allow 5353/udp && sudo ufw allow 27381/udp
```

The firewalld commands above last until reboot. Add `--permanent` to keep
them. If the phone still isn't found, enter the address it shows on screen
on the desktop instead.

## Project layout

| Path | What lives there |
|---|---|
| `apps/desktop` | Desktop app. `src/` is the React and TypeScript interface, organised by feature; `src-tauri/` is a thin Rust shell that only wires commands and events |
| `apps/android` | Android app. A multi-module Gradle build: `app/` is the shell, `core/*` holds shared modules, `feature/*` holds one module per feature |
| `crates/protocol` | Rust types generated from the protobuf schema, plus message framing |
| `crates/core` | Discovery, pairing, the TLS transport, sessions and feature logic, with no interface code |
| `crates/platform` | Operating-system integration (notifications, clipboard, Bluetooth) behind traits, Linux first |
| `proto/link2linux/v1` | The wire format shared by both apps. `proto/testdata` has the byte-level test vectors both apps check against |
| `docs` | [Architecture](docs/architecture.md), the [protocol specification](docs/protocol.md), [decision records](docs/adr) and the [release process](docs/releasing.md) |
| `scripts` | Developer scripts: `doctor.sh` checks your toolchain, `ci.sh` runs the checks |

Each feature has the same name everywhere: `src/features/<name>` on the
desktop, `feature/<name>` on Android, and a module in `crates/core`. That way
one feature reads the same in all three places.

## Checks

Every change must pass the project checks before it's merged. Run them
locally:

```sh
make ci           # every check
make ci-changed   # only the checks for the parts you changed
```

| Job | What it runs |
|---|---|
| `proto` | `buf lint` and `buf breaking` against `main` (or a `protoc` compile without buf) |
| `rust` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, and a check that the generated TypeScript bindings are up to date |
| `web` | ESLint, the TypeScript type check, Vitest and a production build of the desktop interface |
| `android` | `assembleDebug`, unit tests and Android lint |

Other useful targets: `make test`, `make lint`, `make fmt`, `make android`.

## Coding guidelines

- **Follow the existing code.** Match its structure, naming and comment style.
- **Keep the shells thin.** Logic belongs in `crates/core` (desktop) and the
  `core/*` modules (Android), where it can be tested without a window or a
  device.
- **Test new behaviour.** Bug fixes come with a test that would have caught
  the bug.
- **Formatting is automatic.** Run `make fmt` (rustfmt and Prettier). Clippy and
  ESLint warnings fail the checks.
- **Treat permissions carefully.** Every Android permission and desktop
  capability needs a reason in a comment next to it, and features ask for
  permissions only when the user turns them on.
- **Check dependency licenses.** New dependencies must be compatible with
  Apache-2.0. Don't add anything that links GPL code into the apps.

## Changing the protocol

The protobuf schema in `proto/link2linux/v1` is the single source of truth
for the wire format. Both apps generate their code from it.

- **Keep changes backward compatible:** add messages, fields and enum values,
  and never reuse or renumber a field or value. `make ci` runs `buf breaking`
  when buf is installed.
- **Update [`docs/protocol.md`](docs/protocol.md)** in the same pull request, so
  the specification always matches the schema.
- **Regenerate the test vectors** in `proto/testdata` (`generate.sh`,
  `pairing.sh`) if your change affects them. Both the Rust and the Android
  tests read these files.
- **Record significant design decisions** as a new
  [decision record](docs/adr). Don't edit an accepted one; supersede it.

## Commits and pull requests

- **Keep pull requests small and focused**, ideally around 300 changed lines.
  Split larger work into a series of pull requests that build on each other.
  Lock files and generated code don't count towards the size.
- **One logical change per commit, and every commit builds and passes the
  checks.** Put lock file and generated-code updates in their own commits.
- **Write commit subjects as a short imperative sentence** that says what
  changes: "Add the beacon listener", not "beacon stuff". Use the body to
  explain why.
- **In the pull request**, describe what changes and why, and list the checks
  you ran.
- **Expect review.** A maintainer reviews every pull request before it's
  squash-merged.

## Reporting security issues

Please don't report security problems in public issues. Use GitHub's private
reporting instead: open the repository's **Security** tab and choose **Report
a vulnerability**. We'll acknowledge your report and keep you updated while
we work on a fix.

## License

link2linux is licensed under the [Apache License 2.0](LICENSE). By
contributing, you agree that your contributions are licensed under the same
license.
