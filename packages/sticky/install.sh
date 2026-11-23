#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
./build.sh
destination="$HOME/Applications/Sticky.app"
mkdir -p "$HOME/Applications" "$HOME/.local/bin" "$HOME/Library/LaunchAgents"
if [ -x "$destination/Contents/MacOS/sticky" ]; then
  "$destination/Contents/MacOS/sticky" quit
  for attempt in {1..100}; do
    if "$destination/Contents/MacOS/sticky" status | /usr/bin/grep -q '"running" : false'; then break; fi
    sleep 0.05
  done
  if ! "$destination/Contents/MacOS/sticky" status | /usr/bin/grep -q '"running" : false'; then
    printf 'Sticky has not shut down; installation stopped.\n' >&2; exit 1
  fi
fi
stage=$(mktemp -d "$HOME/Applications/.sticky-install.XXXXXX")
trap 'rm -rf "$stage"' EXIT
ditto dist/Sticky.app "$stage/Sticky.app"
codesign --verify --deep --strict "$stage/Sticky.app"
if [ -e "$destination" ]; then mv "$destination" "$stage/previous.app"; fi
mv "$stage/Sticky.app" "$destination"
ln -sfn "$destination/Contents/MacOS/sticky" "$HOME/.local/bin/sticky"
plist="$HOME/Library/LaunchAgents/local.sticky.plist"
cp login.plist "$stage/login.plist"
plutil -insert ProgramArguments.3 -string "$destination" "$stage/login.plist"
plutil -lint "$stage/login.plist" >/dev/null
domain="gui/$(id -u)"
if launchctl print "$domain/local.sticky" >/dev/null 2>&1; then launchctl bootout "$domain/local.sticky"; fi
mv "$stage/login.plist" "$plist"
launchctl enable "$domain/local.sticky"
launchctl bootstrap "$domain" "$plist"
"$HOME/.local/bin/sticky" show
printf 'Installed Sticky. Hyper+N shows/hides notes. Run sticky --help.\n'
