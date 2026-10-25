# 2. One monorepo for desktop, Android, shared crates and schema

Status: accepted

## Context
The desktop app, the Android app and the wire format change together. Keeping
them apart would mean coordinating versions across repositories.

## Decision
A single repository: `apps/desktop` (Tauri + React), `apps/android`
(multi-module Gradle), `crates/` (Rust logic), `proto/` (schema). Feature names
match across all three.

## Consequences
One pull request can change the schema and both implementations. CI builds all
three stacks. The Android project is a standalone Gradle build inside
`apps/android` so Android tooling works unchanged.
