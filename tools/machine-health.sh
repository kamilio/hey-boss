#!/bin/sh
# Manual one-shot cleanup using the standalone harvester.
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_binary="$task_root/target/release/hey-harvester"
if [ ! -x "$task_binary" ]; then
    printf '%s\n' 'Build hey-harvester first: cargo build --locked --release -p hey-harvester' >&2
    exit 1
fi
if [ "$#" -eq 0 ]; then set -- clean; fi
exec "$task_binary" "$@"
