#!/usr/bin/env bash
# Checks the two TLS stacks and pairing implementations against each other:
# the desktop core (Rust, rustls) dials the phone app's listener (Kotlin,
# Conscrypt on the JVM) over loopback and pairs by QR code and by SAS.
#
#   scripts/interop.sh
#
# Needs the Rust toolchain and the Android SDK, like the rust and android
# jobs of scripts/ci.sh, which runs this as its interop job.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

locked=(); [ -f Cargo.lock ] && locked=(--locked)
desktop=$(cargo build "${locked[@]}" -p link2linux-core --example interop_desktop \
  --message-format=json-render-diagnostics |
  sed -n 's/.*"executable":"\([^"]*\/interop_desktop\)".*/\1/p' | tail -n 1)
[ -x "$desktop" ] || { echo "interop: the interop_desktop example was not built"; exit 1; }

cd apps/android
# --rerun: a passing run must never come from Gradle's cache.
L2L_INTEROP_DESKTOP="$desktop" ./gradlew :core:network:testDebugUnitTest \
  --tests 'dev.link2linux.android.core.network.interop.*' --rerun
