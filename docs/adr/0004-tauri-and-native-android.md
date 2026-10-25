# 4. Tauri desktop shell and native Kotlin Android app

Status: accepted

## Context
The desktop app should run on Linux first and other desktop OSes later. The
Android app needs deep system APIs (notification listener, telephony, SMS,
media sessions, foreground services).

## Decision
Desktop: Tauri v2 with a React + TypeScript UI. All logic lives in Rust crates.
Android: native Kotlin with Jetpack Compose.

## Consequences
A small desktop binary that uses the system webview, and a Rust core that is
portable across desktop OSes. Android gets first-class access to platform APIs
without a bridge layer.
