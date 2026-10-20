#!/bin/bash
# Use Launch Services so microphone permission belongs to the app.
set -euo pipefail
cd "$(dirname "$0")"
app="$HOME/Applications/Hey Talk.app"
test -x "$app/Contents/MacOS/hey-talk"
directory="$HOME/Library/LaunchAgents"
plist="$directory/local.hey-talk.plist"
domain="gui/$(id -u)"
mkdir -p "$directory"
stage=$(mktemp "$directory/.hey-talk-login.XXXXXX")
trap 'rm -f "$stage"' EXIT
cp login.plist "$stage"
plutil -insert ProgramArguments.3 -string "$app" "$stage"
plutil -lint "$stage" >/dev/null
chmod 644 "$stage"
if launchctl print "$domain/local.hey-talk" >/dev/null 2>&1; then
  launchctl bootout "$domain/local.hey-talk"
fi
mv "$stage" "$plist"
launchctl enable "$domain/local.hey-talk"
launchctl bootstrap "$domain" "$plist"
for attempt in {1..150}; do
  state=$("$app/Contents/MacOS/hey-talk" status --json | plutil -extract state raw -o - -)
  if [ "$state" != stopped ]; then
    printf 'Hey Talk is running and starts at login.\n'
    exit 0
  fi
  sleep 0.1
done
printf 'Login agent installed, but Hey Talk did not start within 15 seconds.\n' >&2
exit 1
