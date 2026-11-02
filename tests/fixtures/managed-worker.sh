#!/bin/sh
# Private integration fixtures need the actual child PID to test signals and re-exec.
set -eu
: "${HEY_BOSS_ISSUE_DB:?fixture database required}"
binary=$1
shift
id=$(sqlite3 -cmd '.timeout 10000' "$HEY_BOSS_ISSUE_DB" "INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES(lower(hex(randomblob(16))),'cli','{}',1,0) RETURNING id")
export HEY_BOSS_FLEET_MANAGED=1
exec "$binary" worker --id "$id" "$@"
