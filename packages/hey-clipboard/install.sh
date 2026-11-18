#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
./build.sh
destination="$HOME/Applications/Hey Clipboard.app"
state="$HOME/Library/Application Support/hey-clipboard/history.json"
migrate=false
if [ ! -e "$state" ]; then migrate=true; fi
# Flycut writes its in-memory history during graceful termination.
swift -e 'import AppKit
let apps = NSRunningApplication.runningApplications(withBundleIdentifier: "com.generalarcade.flycut")
for app in apps { guard app.terminate() else { exit(1) } }
for _ in 0..<100 { if apps.allSatisfy({ $0.isTerminated }) { exit(0) }; Thread.sleep(forTimeInterval: 0.05) }
exit(1)'
if [ -x "$destination/Contents/MacOS/hey-clipboard" ]; then
  "$destination/Contents/MacOS/hey-clipboard" quit >/dev/null || true
  for attempt in {1..50}; do
    if ! pgrep -f "^$destination/Contents/MacOS/hey-clipboard$" >/dev/null; then break; fi
    sleep 0.1
  done
  if pgrep -f "^$destination/Contents/MacOS/hey-clipboard$" >/dev/null; then
    printf 'Hey Clipboard is still running; installation stopped.\n' >&2
    exit 1
  fi
fi
mkdir -p "$HOME/Applications" "$HOME/.local/bin" "$HOME/Library/LaunchAgents"
staging=$(mktemp -d "$HOME/Applications/.hey-clipboard-install.XXXXXX")
trap 'rm -rf "$staging"' EXIT
ditto 'dist/Hey Clipboard.app' "$staging/Hey Clipboard.app"
codesign --verify --deep --strict "$staging/Hey Clipboard.app"
if [ -e "$destination" ]; then
  mkdir -p "$HOME/.Trash"
  mv "$destination" "$HOME/.Trash/Hey Clipboard-$(date +%Y%m%d-%H%M%S)-$$.app"
fi
mv "$staging/Hey Clipboard.app" "$destination"
ln -sfn "$destination/Contents/MacOS/hey-clipboard" "$HOME/.local/bin/hey-clipboard"
plist="$HOME/Library/LaunchAgents/local.hey-clipboard.plist"
cat > "$staging/login.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>local.hey-clipboard</string>
<key>ProgramArguments</key><array><string>/usr/bin/open</string><string>-g</string><string>-a</string></array>
<key>RunAtLoad</key><true/>
<key>LimitLoadToSessionType</key><string>Aqua</string>
</dict></plist>
PLIST
plutil -insert ProgramArguments.3 -string "$destination" "$staging/login.plist"
plutil -lint "$staging/login.plist" >/dev/null
chmod 644 "$staging/login.plist"
domain="gui/$(id -u)"
if launchctl print "$domain/local.hey-clipboard" >/dev/null 2>&1; then launchctl bootout "$domain/local.hey-clipboard"; fi
mv "$staging/login.plist" "$plist"
launchctl enable "$domain/local.hey-clipboard"
launchctl bootstrap "$domain" "$plist"
for attempt in {1..100}; do
  if "$destination/Contents/MacOS/hey-clipboard" status >/dev/null; then
    if [ "$migrate" = true ] && [ -e "$HOME/Library/Containers/com.generalarcade.flycut/Data/Library/Preferences/com.generalarcade.flycut.plist" ]; then
      "$destination/Contents/MacOS/hey-clipboard" import-flycut
    fi
    printf 'Installed Hey Clipboard. Command+Shift+V opens history.\n'
    exit 0
  fi
  sleep 0.1
done
printf 'Hey Clipboard did not start; check Console.\n' >&2
exit 1
