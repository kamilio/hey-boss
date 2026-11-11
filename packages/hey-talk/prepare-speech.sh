#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
manifest="$PWD/Sources/HeyTalk/speech-model.json"
destination="$HOME/Library/Application Support/hey-talk/models/$(jq -er .name "$manifest")"
checksums=$(jq -er '.files[] | .sha256 + "  " + (.url | split("/") | last)' "$manifest")
verify() { ( cd "$1" && shasum -a 256 -c - <<< "$checksums" >/dev/null ); }
if [ -d "$destination" ] && verify "$destination"; then
  cp pocket-tts.yaml "$destination/config.yaml"
  cp Licenses/PocketTTS-Weights.txt "$destination/LICENSE-WEIGHTS"
  cp THIRD_PARTY.md "$destination/NOTICE.md"
  exit 0
fi
mkdir -p "$(dirname "$destination")"
stage=$(mktemp -d "${destination}.staging.XXXXXX")
trap 'rm -rf "$stage"' EXIT
urls=$(jq -er '.files[].url' "$manifest")
while IFS= read -r url; do
  curl -fL --retry 2 -o "$stage/${url##*/}" "$url"
done <<< "$urls"
verify "$stage"
cp pocket-tts.yaml "$stage/config.yaml"
cp Licenses/PocketTTS-Weights.txt "$stage/LICENSE-WEIGHTS"
cp THIRD_PARTY.md "$stage/NOTICE.md"
if [ -d "$destination" ]; then mv "$destination" "${destination}.previous-$(date +%s)"; fi
mv "$stage" "$destination"
printf 'Speech model ready: %s\n' "$destination"
