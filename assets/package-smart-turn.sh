#!/bin/bash
# Fetch verified build inputs once; the installed detector never uses the network.
set -euo pipefail
contents=$1
cache="$HOME/.cache/hey-boss/smart-turn"
mkdir -p "$cache" "$contents/Frameworks" "$contents/Resources"
stage=$(mktemp -d "$cache/build.XXXXXX")
trap 'rm -rf "$stage"' EXIT
fetch() {
  local file="$cache/$1" sha=$2 url=$3
  if ! printf '%s  %s\n' "$sha" "$file" | shasum -a 256 -c - >/dev/null 2>&1; then
    curl -fLsS --retry 2 "$url" -o "$stage/$1"
    printf '%s  %s\n' "$sha" "$stage/$1" | shasum -a 256 -c -
    mv "$stage/$1" "$file"
  fi
}
fetch smart-turn-v3.2.onnx 2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f \
  https://raw.githubusercontent.com/pipecat-ai/pipecat/a28e1f2d346407c3cfb0fbf3ee346a0307c52071/src/pipecat/audio/turn/smart_turn/data/smart-turn-v3.2-cpu.onnx
fetch sherpa-v1.13.8.tar.bz2 2249f97f10df7d828af1b0d10e3d1eeaa2198b60b9a999a7ed853c150181ccfd \
  https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-osx-universal2-shared.tar.bz2
cp "$cache/smart-turn-v3.2.onnx" "$contents/Resources/smart-turn.onnx"
tar -xOjf "$cache/sherpa-v1.13.8.tar.bz2" sherpa-onnx-v1.13.8-osx-universal2-shared/lib/libonnxruntime.dylib >"$stage/libonnxruntime.dylib"
lipo "$stage/libonnxruntime.dylib" -thin "$(uname -m)" -output "$contents/Frameworks/libonnxruntime.dylib"
