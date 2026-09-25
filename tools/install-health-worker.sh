#!/bin/sh
# Compatibility entry point for the standalone harvester installer.
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$task_root/install-harvester.sh" "$@"
