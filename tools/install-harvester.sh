#!/bin/bash
# Build and publish only hey-harvester. Never restart Hey Boss or issue web.
set -euo pipefail
task_host=${1:?Usage: install-harvester.sh local|SSH_HOST}
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_ssh=(-T -o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=10 -o ServerAliveCountMax=2)
if [[ -n ${HEY_BOSS_SSH_CONTROL_PATH:-} ]]; then task_ssh+=(-S "$HEY_BOSS_SSH_CONTROL_PATH"); fi
if [[ $task_host == -* || $task_host == *[!a-zA-Z0-9@._:\[\]-]* ]]; then printf '%s\n' 'Invalid SSH host' >&2; exit 1; fi
task_run() {
    if [[ $task_host == local ]]; then sh -c "$1"; else ssh "${task_ssh[@]}" "$task_host" "$1"; fi
}
# The workspace manifest references the siblings, but Cargo builds only harvester.
COPYFILE_DISABLE=1 tar --no-xattrs --exclude=target --exclude=node_modules -czf - -C "$task_root" Cargo.toml Cargo.lock build.rs src packages/hey-gh packages/hey-harvester |
    task_run 'set -eu
umask 077
stage=$(mktemp -d)
trap '\''rm -rf "$stage"'\'' EXIT HUP INT TERM
tar -xzf - -C "$stage"
# Archives preserve old mtimes; force Cargo to inspect this source revision
# instead of reusing a newer artifact from another temporary source tree.
touch "$stage/packages/hey-harvester/src/lib.rs"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
export CARGO_TARGET_DIR="$HOME/.cache/hey-harvester/build"
export CARGO_BUILD_JOBS=2
cargo build --locked --release -p hey-harvester --manifest-path "$stage/Cargo.toml"
"$CARGO_TARGET_DIR/release/hey-harvester" install
'
