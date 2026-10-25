#!/bin/sh
# Installs the link2linux desktop app from the newest GitHub release.
#
#   curl -fsSL https://codesensei-tushar.github.io/link2linux/install.sh | sh
#
# It installs the .deb on apt systems, the .rpm on dnf and zypper systems,
# and the AppImage everywhere else. Every download is checked against the
# release's SHA256SUMS before anything is installed.
#
# Options (with curl, pass them as: ... | sh -s -- --appimage):
#   --appimage   install the AppImage even where a package would fit
#   --dry-run    show what would be installed, and change nothing
# Environment:
#   L2L_VERSION  install this release tag instead of the newest one
set -eu

# Everything runs from main, called on the last line, so a download cut off
# halfway through runs nothing.
main() {
repo="codesensei-tushar/link2linux"
format=""
dry_run=false

say() { printf '%s\n' "$*"; }
die() { printf 'link2linux install: %s\n' "$*" >&2; exit 1; }
run() { if $dry_run; then say "would run: $*"; else "$@"; fi; }

for arg in "$@"; do
  case "$arg" in
    --appimage) format=appimage ;;
    --dry-run) dry_run=true ;;
    *) die "unknown option: $arg" ;;
  esac
done

command -v curl >/dev/null || die "curl is required"
[ "$(uname -s)" = Linux ] || die "the desktop app runs on Linux only"

case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 deb_arch=amd64 ;;
  aarch64 | arm64) arch=aarch64 deb_arch=arm64 ;;
  *) die "no build for this CPU architecture: $(uname -m)" ;;
esac

if [ -z "$format" ]; then
  if command -v apt-get >/dev/null && command -v dpkg >/dev/null; then format=deb
  elif command -v dnf >/dev/null; then format=dnf
  elif command -v zypper >/dev/null; then format=zypper
  else format=appimage
  fi
fi
case "$format" in
  deb) asset="link2linux_${deb_arch}.deb" ;;
  dnf | zypper) asset="link2linux.${arch}.rpm" ;;
  appimage) asset="link2linux-${arch}.AppImage" ;;
esac

tag="${L2L_VERSION:-}"
if [ -z "$tag" ]; then
  # The newest release, pre-releases included. The API pretty-prints its
  # JSON, so the first "tag_name" line belongs to the newest release.
  releases=$(curl -fsSL "https://api.github.com/repos/$repo/releases?per_page=1") ||
    die "could not reach GitHub to find the newest release"
  tag=$(printf '%s\n' "$releases" | sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
  [ -n "$tag" ] || die "there is no release yet. See https://github.com/$repo/releases"
fi
base="https://github.com/$repo/releases/download/$tag"

sudo=""
if [ "$format" != appimage ] && [ "$(id -u)" -ne 0 ]; then
  command -v sudo >/dev/null || die "sudo is needed to install the package; or use --appimage"
  sudo=sudo
fi

say "Installing link2linux $tag: $asset"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM
file="$tmp/$asset"
if $dry_run; then
  say "would download: $base/$asset and SHA256SUMS, and check the checksum"
else
  curl -fL --progress-bar -o "$file" "$base/$asset" || die "download failed: $base/$asset"
  curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "download failed: $base/SHA256SUMS"
  asset_re=$(printf '%s' "$asset" | sed 's/[.]/\\./g')
  (cd "$tmp" && grep -E "^[0-9a-f]{64} [ *]$asset_re\$" SHA256SUMS | sha256sum -c - >/dev/null) ||
    die "the checksum of $asset does not match SHA256SUMS; nothing was installed"
  say "Checksum verified."
fi

case "$format" in
  deb) run $sudo apt-get install -y "$file" ;;
  dnf) run $sudo dnf install -y "$file" ;;
  zypper) run $sudo zypper --non-interactive install --allow-unsigned-rpm "$file" ;;
  appimage)
    dest="${XDG_BIN_HOME:-$HOME/.local/bin}"
    run mkdir -p "$dest"
    run install -m 0755 "$file" "$dest/link2linux"
    case ":$PATH:" in
      *":$dest:"*) ;;
      *) say "Add $dest to your PATH to start link2linux from a terminal." ;;
    esac
    ;;
esac

if [ "$format" = appimage ]; then
  say "Done. Start it with: link2linux"
else
  say "Done. Start link2linux from your app menu."
fi
}

main "$@"
