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
staging_root=$(mktemp -d "$HOME/Applications/.hey-talk-install.XXXXXX")
stage="$staging_root/Hey Talk.app"
ditto 'dist/Hey Talk.app' "$stage"
codesign --verify --deep --strict "$stage"
if [ -e "$destination" ]; then
  mkdir -p "$HOME/.Trash"
  previous="$HOME/.Trash/Hey Talk-$(date +%Y%m%d-%H%M%S)-$$.app"
  mv "$destination" "$previous"
fi
mv "$stage" "$destination"
rmdir "$staging_root"
ln -sfn "$destination/Contents/MacOS/hey-talk" "$HOME/.local/bin/hey-talk"
printf 'Installed %s\nRun ~/.local/bin/hey-talk --help. Nothing starts at login.\n' "$destination"
