#!/bin/sh
# Enroll this machine only. Normal upgrades stage, verify and roll back the CLI.
set -eu
marker="$HOME/.config/hey-boss/git-shelf"
if [ ! -e "$marker" ] && { [ -e "$HOME/.local/bin/g" ] || [ -L "$HOME/.local/bin/g" ]; }; then
    echo "Another command owns ~/.local/bin/g. Move it before enrolling the shelf." >&2
    exit 1
fi
mkdir -p "$HOME/.config/hey-boss" "$HOME/.local/bin"
touch "$marker"
exec hey-boss upgrade --local-only
