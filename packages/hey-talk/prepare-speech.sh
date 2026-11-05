#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
destination="$HOME/Library/Application Support/hey-talk/models/pocket-tts-english-2026-10"
manifest="$PWD/speech-model.sha256"
verify() { ( cd "$1" && shasum -a 256 -c "$manifest" >/dev/null ); }
if [ -d "$destination" ] && verify "$destination"; then
  cp pocket-tts.yaml "$destination/config.yaml"
  cp Licenses/PocketTTS-Weights.txt "$destination/LICENSE-WEIGHTS"
  cp THIRD_PARTY.md "$destination/NOTICE.md"
  exit 0
fi
mkdir -p "$(dirname "$destination")"
stage=$(mktemp -d "${destination}.staging.XXXXXX")
trap 'rm -rf "$stage"' EXIT
base=https://huggingface.co/kyutai/pocket-tts-without-voice-cloning/resolve
curl -fL --retry 2 -o "$stage/model.safetensors" "$base/a7056f505a4a8ca72d8b5fb1440071d6f1a35739/languages/english/model.safetensors"
curl -fL --retry 2 -o "$stage/tokenizer.json" "$base/00eac05ed3d16bdc3f6b5d598874019c34a89214/languages/english/tokenizer.json"
curl -fL --retry 2 -o "$stage/mary.safetensors" "$base/a3055dcb533cf2ec90458008a01e7cef550bae2a/languages/english/embeddings/mary.safetensors"
verify "$stage"
cp pocket-tts.yaml "$stage/config.yaml"
cp Licenses/PocketTTS-Weights.txt "$stage/LICENSE-WEIGHTS"
cp THIRD_PARTY.md "$stage/NOTICE.md"
if [ -d "$destination" ]; then mv "$destination" "${destination}.previous-$(date +%s)"; fi
mv "$stage" "$destination"
printf 'Pocket TTS with Mary ready: %s\n' "$destination"
