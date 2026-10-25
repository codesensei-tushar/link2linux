# 3. Protobuf as the wire format

Status: accepted

## Context
Rust and Kotlin must agree on every message. Hand-written JSON on both sides
drifts and has no compatibility rules.

## Decision
Messages are defined once in `proto/link2linux/v1`. Rust generates types with
prost, Kotlin with the Gradle protobuf plugin. `buf` lints the schema and checks
for breaking changes in CI.

## Consequences
Compact binary frames and generated, typed code on both sides. Compatibility
follows protobuf rules: add fields, never reuse tags.
