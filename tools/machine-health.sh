#!/bin/sh
# Manual one-shot cleanup; the same backend powers the native Health window.
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_binary=${HEY_BOSS_CLI_PATH:-"$task_root/target/release/hey-boss"}
if [ ! -x "$task_binary" ]; then
    printf '%s\n' 'Build hey-boss first: cargo build --locked --release' >&2
    exit 1
fi
if [ "$#" -eq 0 ]; then set -- clean; fi
exec "$task_binary" health "$@"
