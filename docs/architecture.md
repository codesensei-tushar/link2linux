# Architecture

link2linux has two apps that talk to each other over one secure session:
a desktop app (Linux first, other desktop OSes later) and an Android app.

```
 Android app (Kotlin)                         Desktop app
 ┌──────────────────────┐                     ┌──────────────────────────────┐
 │ feature/<name>       │                     │ React UI  src/features/<name> │
 │ core/data, network   │                     │        │ typed IPC            │
 │ core/protocol ◄──────┼── protobuf frames ──┼─► src-tauri (thin shell)      │
 └──────────────────────┘   over TLS (LAN)    │        │                      │
                                              │ crates/core   crates/platform │
                                              │ crates/protocol               │
                                              └──────────────────────────────┘
```

## Principles

- **One schema.** `proto/link2linux/v1` is the only definition of the wire
  format. Rust (prost) and Kotlin generate their types from it.
- **Thin shells.** `src-tauri` only wires commands, state and events. All
  real desktop logic sits in `crates/` so it can be tested without a window.
- **OS behind traits.** `crates/platform` defines traits (notifications,
  Bluetooth, audio, files). `platform::linux` implements them, and other
  OSes slot in beside it.
- **Same feature names everywhere.** pairing, notifications, messages, calls,
  photos, files, clipboard, screen.
- **Opt-in permissions.** Each phone feature is off until the user turns it
  on, and Android permissions are requested at that moment.

## Components

| Component | Responsibility |
|---|---|
| `crates/protocol` | Generated message types, framing (length-delimited) |
| `crates/core` | Discovery, pairing, TLS transport, session, per-feature services |
| `crates/platform` | Traits + Linux implementation (D-Bus, BlueZ, PipeWire, ...) |
| `apps/desktop/src-tauri` | Tauri commands/events bridging UI ↔ core |
| `apps/desktop/src` | UI, organized by feature |
| `apps/android/core/*` | model, protocol, network, data, designsystem, common |
| `apps/android/feature/*` | One module per feature |

The protocol itself is specified in [protocol.md](protocol.md). Decisions
are recorded in [adr/](adr/).
