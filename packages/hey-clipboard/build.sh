#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
swift build -c release
app="$PWD/dist/Hey Clipboard.app"
mkdir -p "$app/Contents/MacOS"
cp .build/release/hey-clipboard "$app/Contents/MacOS/hey-clipboard"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.hey-clipboard</string>
<key>CFBundleName</key><string>Hey Clipboard</string>
<key>CFBundleDisplayName</key><string>Hey Clipboard</string>
<key>CFBundleExecutable</key><string>hey-clipboard</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>15.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
# Share the repo's persistent signing identity so updates retain macOS trust.
identity=$(tr -d '[:space:]' < ../hey-talk/signing-identity.sha1)
codesign --force --sign "$identity" --timestamp=none --options runtime "$app"
codesign --verify --deep --strict -R "=identifier \"local.hey-clipboard\" and certificate leaf = H\"$identity\"" "$app"
printf 'Built %s\n' "$app"
