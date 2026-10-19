#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
destination="$HOME/Library/Application Support/hey-talk/models/orukeet-v0.1.0"
source="$HOME/.cache/openwhispr/parakeet-models/orukeet-v0.1.0"
manifest="$PWD/model.sha256"
verify() { ( cd "$1" && shasum -a 256 -c "$manifest" >/dev/null ); }
if [ -d "$destination" ] && verify "$destination"; then exit 0; fi
mkdir -p .build/vendor
if [ ! -d "$source" ] || ! verify "$source"; then
  archive="$PWD/.build/vendor/orukeet.tar.bz2"
  curl -fL --retry 2 -o "$archive.tmp" 'https://huggingface.co/oruk/orukeet/resolve/55a984d46f68323301837194ce647c702f55facc/onnx/sherpa-onnx-orukeet-v0.1.0-int8.tar.bz2'
  printf '%s  %s\n' 'f9191f30178cc9122ce2f023bf9fefafc822028307b0efa4caff645ba3fe8d0a' "$archive.tmp" | shasum -a 256 -c -
  mv "$archive.tmp" "$archive"
  tar -xjf "$archive" -C .build/vendor
  source="$PWD/.build/vendor/sherpa-onnx-orukeet-v0.1.0-int8"
  verify "$source"
fi
mkdir -p "$(dirname "$destination")"
stage="${destination}.staging"
rm -rf "$stage"
mkdir -p "$stage"
for name in encoder.int8.onnx decoder.int8.onnx joiner.int8.onnx tokens.txt LICENSE-WEIGHTS NOTICE.md; do
  cp -c "$source/$name" "$stage/$name" 2>/dev/null || cp "$source/$name" "$stage/$name"
done
verify "$stage"
if [ -d "$destination" ]; then mv "$destination" "${destination}.previous-$(date +%s)"; fi
mv "$stage" "$destination"
printf 'Orukeet ready: %s\n' "$destination"
