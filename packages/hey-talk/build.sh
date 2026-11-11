#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p .build/vendor dist
vendor="$PWD/.build/vendor"
archive="$vendor/sherpa-tts.tar.bz2"
runtime="$vendor/sherpa-onnx-v1.13.8-osx-universal2-shared"
if [ ! -f "$archive" ]; then
  curl -fL --retry 2 -o "$archive.tmp" 'https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-osx-universal2-shared.tar.bz2'
  mv "$archive.tmp" "$archive"
fi
printf '%s  %s\n' '2249f97f10df7d828af1b0d10e3d1eeaa2198b60b9a999a7ed853c150181ccfd' "$archive" | shasum -a 256 -c -
if [ ! -d "$runtime" ]; then tar -xjf "$archive" -C "$vendor"; fi
swift build -c release
app="$PWD/dist/Hey Talk.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Frameworks" "$app/Contents/Resources"
cp .build/release/hey-talk "$app/Contents/MacOS/hey-talk"
cp .build/release/hey-talk "$app/Contents/Helpers/hey-talk-inference"
case "$(uname -m)" in
  arm64) pocket_arch=aarch64; pocket_sha=ee9ca6c7c000ee72c5846e3a8ba01d08f56636d94a614d2259c3f12b8fd69afa ;;
  x86_64) pocket_arch=x86_64; pocket_sha=f04baa94d4fc47a689cc9f11ca2b0ba869dc002f9e78e15d387c6ecaeb1d654b ;;
  *) printf 'Unsupported Pocket TTS architecture\n' >&2; exit 1 ;;
esac
pocket="pocket-tts-3.3.0-$pocket_arch-apple-darwin"
if ! printf '%s  %s\n' "$pocket_sha" "$vendor/$pocket.tar.gz" | shasum -a 256 -c - >/dev/null 2>&1; then
  curl -fL --retry 2 -o "$vendor/$pocket.tar.gz.download" "https://github.com/kyutai-labs/pocket-tts/releases/download/v4.0.1/$pocket.tar.gz"
  printf '%s  %s\n' "$pocket_sha" "$vendor/$pocket.tar.gz.download" | shasum -a 256 -c -
  mv "$vendor/$pocket.tar.gz.download" "$vendor/$pocket.tar.gz"
fi
tar -xzf "$vendor/$pocket.tar.gz" -C "$vendor"
cp "$vendor/$pocket/pocket-tts" "$app/Contents/Helpers/pocket-tts"
cp "$runtime/lib/libsherpa-onnx-c-api.dylib" "$app/Contents/Frameworks/"
cp "$runtime/lib/libonnxruntime.dylib" "$app/Contents/Frameworks/"
if [ "$(uname -m)" = arm64 ]; then
  # The arm64-specific ONNX Runtime is substantially faster on INT8 models
  # than the universal release. Same optimization used by OpenWhispr.
  ort="$vendor/onnxruntime-arm64.zip"
  if [ ! -f "$ort" ]; then
    curl -fL --retry 2 -o "$ort.tmp" 'https://github.com/csukuangfj/onnxruntime-libs/releases/download/v1.28.2/onnxruntime-osx-arm64-1.28.2.zip'
    mv "$ort.tmp" "$ort"
  fi
  printf '%s  %s\n' 'd9e5c0c79929e201f5b8eb095e6809a91ce78be9866a1bca0dfab7e20b40ae40' "$ort" | shasum -a 256 -c -
  unzip -oq "$ort" -d "$vendor"
  cp "$vendor/onnxruntime-osx-arm64-1.28.2/lib/libonnxruntime.dylib" "$app/Contents/Frameworks/libonnxruntime.dylib"
fi
install_name_tool -change '@rpath/libonnxruntime.dylib' '@loader_path/libonnxruntime.dylib' "$app/Contents/Frameworks/libsherpa-onnx-c-api.dylib"
cp THIRD_PARTY.md "$app/Contents/Resources/"
cp vocabulary.md Sources/HeyTalk/speech-model.json "$app/Contents/Resources/"
cp Sources/HeyTalk/Prompts/*.md "$app/Contents/Resources/"
cp -R Licenses "$app/Contents/Resources/"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.hey-talk</string>
<key>CFBundleName</key><string>Hey Talk</string>
<key>CFBundleDisplayName</key><string>Hey Talk</string>
<key>CFBundleExecutable</key><string>hey-talk</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>15.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
<key>NSMicrophoneUsageDescription</key><string>Hey Talk records your microphone while you dictate and transcribes it locally.</string>
</dict></plist>
PLIST
for library in "$app/Contents/Frameworks/"*.dylib; do codesign --force --sign - "$library"; done
# The worker loads ad-hoc-signed native libraries and has no UI, microphone,
# or Accessibility privileges. Keep the desktop executable hardened.
codesign --force --sign - "$app/Contents/Helpers/hey-talk-inference"
codesign --force --sign - "$app/Contents/Helpers/pocket-tts"
./sign-app.sh "$app"
printf '\nBuilt: %s\nCLI: %s/Contents/MacOS/hey-talk --help\n' "$app" "$app"
