#!/usr/bin/env bash
# Report which parts of the toolchain are present.
set -u
ok=0
check() { if eval "$2" >/dev/null 2>&1; then printf '  ok   %s\n' "$1"; else printf '  MISS %s  (%s)\n' "$1" "$3"; ok=1; fi; }

echo "toolchain:"
check "rustc"          "command -v rustc"            "install rustup"
check "cargo"          "command -v cargo"            "install rustup"
check "node >= 24"     "node -e 'process.exit(+process.versions.node.split(\".\")[0] >= 24 ? 0 : 1)'" "install Node 24"
check "pnpm"           "command -v pnpm"             "corepack enable"
check "java 17+"       "java -version"               "install a JDK 17"
check "ANDROID_HOME"   "test -d \"\${ANDROID_HOME:-}\"" "install the Android SDK and export ANDROID_HOME"
check "protoc"         "command -v protoc"           "apt install protobuf-compiler"
echo "tauri linux libraries:"
for lib in webkit2gtk-4.1 ayatana-appindicator3-0.1 librsvg-2.0; do
  check "$lib" "pkg-config --exists $lib" "apt install the -dev package"
done
check "libxdo" "test -e /usr/include/xdo.h" "apt install libxdo-dev"
exit $ok
