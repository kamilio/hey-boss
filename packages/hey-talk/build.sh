#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p .build/vendor dist
vendor="$PWD/.build/vendor"
archive="$vendor/sherpa.tar.bz2"
runtime="$vendor/sherpa-onnx-v1.13.8-osx-universal2-shared-no-tts"
if [ ! -f "$archive" ]; then
  curl -fL --retry 2 -o "$archive.tmp" 'https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-osx-universal2-shared-no-tts.tar.bz2'
  mv "$archive.tmp" "$archive"
fi
printf '%s  %s\n' '636b439c75031d892636dfdd2000a0233e4a30a08b030697e91f8aa389431b83' "$archive" | shasum -a 256 -c -
if [ ! -d "$runtime" ]; then tar -xjf "$archive" -C "$vendor"; fi
swift build -c release
app="$PWD/dist/Hey Talk.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Frameworks" "$app/Contents/Resources"
cp .build/release/hey-talk "$app/Contents/MacOS/hey-talk"
cp .build/release/hey-talk "$app/Contents/Helpers/hey-talk-inference"
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
codesign --force --sign - --options runtime --entitlements entitlements.plist "$app"
codesign --verify --deep --strict "$app"
printf '\nBuilt: %s\nCLI: %s/Contents/MacOS/hey-talk --help\n' "$app" "$app"
