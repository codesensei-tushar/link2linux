# link2linux

Connect your Android phone to your Linux desktop: notifications, messages,
calls, photos, files, clipboard and screen, over your local network.

> Status: early development. Nothing here is usable yet.

## Layout

| Path | What lives there |
|---|---|
| `apps/desktop` | Desktop app: Tauri v2 shell + React/TypeScript UI |
| `apps/android` | Android app: Kotlin, multi-module Gradle build |
| `crates/protocol` | Wire-format types generated from `proto/` |
| `crates/core` | Discovery, pairing, secure transport, sessions, feature services |
| `crates/platform` | OS integration behind traits (Linux first) |
| `proto/link2linux/v1` | Protobuf schema shared by desktop and phone |
| `docs/` | Architecture, protocol spec, decision records (`docs/adr`) |

Each feature uses the same name everywhere: `src/features/<name>` on the
desktop, `feature/<name>` on Android, and a module in `crates/core`.

## Development

Requirements: Rust (pinned in `rust-toolchain.toml`), Node 24 + pnpm,
JDK 17, Android SDK (`ANDROID_HOME`), and the Tauri Linux libraries
(`libwebkit2gtk-4.1-dev`, `libxdo-dev`, `libayatana-appindicator3-dev`,
`librsvg2-dev`). `scripts/doctor.sh` checks all of them.

```sh
make doctor        # check the toolchain
make dev           # run the desktop app with hot reload
make test          # Rust, frontend and Android tests
make lint          # fmt/clippy, eslint/tsc, Android lint
make android       # build the debug APK
```

## License

Apache-2.0. See [LICENSE](LICENSE).
