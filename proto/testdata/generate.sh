#!/usr/bin/env bash
# Regenerates the byte-level test vectors from their text-format sources.
#
# Each vNN/*.txtpb file holds one Envelope. protoc, the reference encoder,
# serializes it; this script adds the varint length prefix and appends the
# frame to vNN/frames.bin, in file name order. The vectors are committed, so
# only run this after changing a source on purpose.
set -euo pipefail
cd "$(dirname "$0")"

# Appends one byte, given as a number, to the file named by $1.
put_byte() { printf "\\x$(printf %02x "$2")" >> "$1"; }

for dir in v*/; do
  out="${dir}frames.bin"
  : > "$out"
  for src in "${dir}"*.txtpb; do
    body=$(mktemp)
    protoc --proto_path=.. --encode=link2linux.v1.Envelope \
      link2linux/v1/envelope.proto < "$src" > "$body"
    len=$(wc -c < "$body")
    while (( len >= 0x80 )); do
      put_byte "$out" $(( (len & 0x7f) | 0x80 ))
      len=$(( len >> 7 ))
    done
    put_byte "$out" "$len"
    cat "$body" >> "$out"
    rm -f "$body"
  done
  echo "wrote $out ($(wc -c < "$out") bytes)"
done
