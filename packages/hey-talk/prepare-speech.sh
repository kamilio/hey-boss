#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
destination="$HOME/Library/Application Support/hey-talk/models/sherpa-onnx-supertonic-3-tts-int8-2026-05-11"
manifest="$PWD/speech-model.sha256"
verify() { ( cd "$1" && shasum -a 256 -c "$manifest" >/dev/null ); }
if [ -d "$destination" ] && verify "$destination"; then exit 0; fi
mkdir -p .build/vendor
archive="$PWD/.build/vendor/supertonic.tar.bz2"
checksum=82fa96f91c4ef8abaae3a14a3f4153facf88bed821d1f7331cec2700f432c427
if ! printf '%s  %s\n' "$checksum" "$archive" | shasum -a 256 -c - >/dev/null 2>&1; then
  curl -fL --retry 2 -o "$archive.tmp" 'https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/sherpa-onnx-supertonic-3-tts-int8-2026-05-11.tar.bz2'
  printf '%s  %s\n' "$checksum" "$archive.tmp" | shasum -a 256 -c -
  mv "$archive.tmp" "$archive"
fi
mkdir -p "$(dirname "$destination")"
stage=$(mktemp -d "${destination}.staging.XXXXXX")
trap 'rm -rf "$stage"' EXIT
tar -xjf "$archive" -C "$stage" --strip-components=1
verify "$stage"
if [ -d "$destination" ]; then mv "$destination" "${destination}.previous-$(date +%s)"; fi
mv "$stage" "$destination"
printf 'Supertonic ready: %s\n' "$destination"
