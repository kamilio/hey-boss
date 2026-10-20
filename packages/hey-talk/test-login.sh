#!/bin/bash
# Exercise the installed login job while dictation is idle. No microphone or paste.
set -euo pipefail
cd "$(dirname "$0")"
cli="$HOME/Applications/Hey Talk.app/Contents/MacOS/hey-talk"
job="gui/$(id -u)/local.hey-talk"
state=$("$cli" status --json | plutil -extract state raw -o - -)
if [ "$state" = recording ] || [ "$state" = processing ]; then
  printf 'Finish or cancel dictation before testing login startup.\n' >&2
  exit 1
fi
./install-login.sh
./install-login.sh # Reinstalling must work with an already loaded job.
launchctl print "$job" >/dev/null
plist="$HOME/Library/LaunchAgents/local.hey-talk.plist"
test "$(plutil -extract RunAtLoad raw -o - "$plist")" = true
test "$(plutil -extract LimitLoadToSessionType raw -o - "$plist")" = Aqua
test "$(plutil -extract ProgramArguments raw -o - "$plist")" = 4
if plutil -extract KeepAlive raw -o - "$plist" >/dev/null 2>&1; then
  printf 'Login startup must allow explicit quit.\n' >&2
  exit 1
fi
"$cli" quit
for attempt in {1..100}; do
  state=$("$cli" status --json | plutil -extract state raw -o - -)
  if [ "$state" = stopped ]; then break; fi
  sleep 0.05
done
test "$state" = stopped
sleep 2
test "$("$cli" status --json | plutil -extract state raw -o - -)" = stopped
# Run the same job launchd runs at login. status never launches the app itself.
launchctl kickstart "$job"
for attempt in {1..150}; do
  status=$("$cli" status --json)
  if [ "$(printf '%s' "$status" | plutil -extract state raw -o - -)" != stopped ]; then break; fi
  sleep 0.1
done
test "$(printf '%s' "$status" | plutil -extract state raw -o - -)" = idle
test "$(printf '%s' "$status" | plutil -extract hotkey_registered raw -o - -)" = true
test "$(printf '%s' "$status" | plutil -extract model_ready raw -o - -)" = true
printf 'PASS: login job starts dictation and shortcuts; explicit quit stays stopped.\n'
