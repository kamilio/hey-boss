#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release --manifest-path renderer/Cargo.toml --target-dir renderer/target
(cd web && npm ci --no-audit --no-fund && npm run build)
swift build -c release
app="$PWD/dist/Sticky.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp .build/release/sticky "$app/Contents/MacOS/sticky"
rm -rf "$app/sticky_Sticky.bundle"
cp Sources/Sticky/Resources/* "$app/Contents/Resources/"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.sticky</string>
<key>CFBundleName</key><string>Sticky</string>
<key>CFBundleDisplayName</key><string>Sticky</string>
<key>CFBundleExecutable</key><string>sticky</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>26.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$app"
printf '\nBuilt %s\n' "$app"
