#!/bin/bash
# Explicit, on-demand installation on the current Mac. No launch agent.
set -euo pipefail
cd "$(dirname "$0")"
./build.sh
./prepare-model.sh
destination="$HOME/Applications/Hey Talk.app"
mkdir -p "$HOME/Applications" "$HOME/.local/bin"
if [ -e "$destination" ]; then
  "$destination/Contents/MacOS/hey-talk" quit
fi
stage="$HOME/Applications/Hey Talk.staging.app"
rm -rf "$stage"
ditto 'dist/Hey Talk.app' "$stage"
codesign --verify --deep --strict "$stage"
previous="$HOME/Applications/Hey Talk.previous.app"
if [ -e "$destination" ]; then
  rm -rf "$previous"
  mv "$destination" "$previous"
fi
mv "$stage" "$destination"
ln -sfn "$destination/Contents/MacOS/hey-talk" "$HOME/.local/bin/hey-talk"
printf 'Installed %s\nRun ~/.local/bin/hey-talk --help. Nothing starts at login.\n' "$destination"
