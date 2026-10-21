#!/bin/bash
# Install on the current Mac and start at login.
set -euo pipefail
cd "$(dirname "$0")"
./build.sh
./prepare-model.sh
./prepare-speech.sh
destination="$HOME/Applications/Hey Talk.app"
mkdir -p "$HOME/Applications" "$HOME/.local/bin"
if [ -e "$destination" ]; then
  status=$("$destination/Contents/MacOS/hey-talk" status --json)
  state=$(printf '%s' "$status" | plutil -extract state raw -o - -)
  if [ "$state" = recording ] || [ "$state" = processing ]; then
    printf 'Finish or cancel the current dictation before installing.\n' >&2
    exit 1
  fi
  previous_pid=$(printf '%s' "$status" | plutil -extract pid raw -o - - 2>/dev/null || true)
  "$destination/Contents/MacOS/hey-talk" quit
  if [[ "$previous_pid" =~ ^[0-9]+$ ]]; then
    for attempt in {1..100}; do
      if ! kill -0 "$previous_pid" 2>/dev/null; then break; fi
      sleep 0.05
    done
    if kill -0 "$previous_pid" 2>/dev/null; then
      printf 'Hey Talk has not finished shutting down; installation stopped.\n' >&2
      exit 1
    fi
  fi
fi
staging_root=$(mktemp -d "$HOME/Applications/.hey-talk-install.XXXXXX")
stage="$staging_root/Hey Talk.app"
ditto 'dist/Hey Talk.app' "$stage"
codesign --verify --deep --strict "$stage"
identity=$(tr -d '[:space:]' < signing-identity.sha1)
codesign --verify --strict -R "=identifier \"local.hey-talk\" and certificate leaf = H\"$identity\"" "$stage"
if [ -e "$destination" ]; then
  mkdir -p "$HOME/.Trash"
  previous="$HOME/.Trash/Hey Talk-$(date +%Y%m%d-%H%M%S)-$$.app"
  mv "$destination" "$previous"
fi
mv "$stage" "$destination"
rmdir "$staging_root"
ln -sfn "$destination/Contents/MacOS/hey-talk" "$HOME/.local/bin/hey-talk"
./install-login.sh
printf 'Installed %s\nRun ~/.local/bin/hey-talk --help.\n' "$destination"
