#!/bin/bash
# Usage: tools/spoken_review_checks.sh /absolute/path/to/hey-boss /absolute/path/to/hey-boss-test
set -eu
scripts=$(cd "$(dirname "$0")" && pwd)
cli=$1
audit=$2
mkdir -p out
root=$(mktemp -d "$PWD/out/spoken-review.XXXXXX")
root=$(cd "$root" && pwd -P)
key=$(printf %s "$root/issues.db" | shasum -a 256 | cut -c1-24)
socket="/tmp/hey-boss-db-$(id -u)/$key"
export HEY_BOSS_CLI_PATH="$cli" HEY_BOSS_ISSUE_DB="$root/issues.db" HEY_BOSS_FLEET_STATE="$root" HEY_BOSS_FLEET_CONFIG="$root/inventory.json" HEY_BOSS_FLEET_DESIRED="$root/desired.json" HEY_BOSS_INBOX_SOCKET="$root/inbox.sock" HEY_BOSS_AGENT_ID=human:verification
unset HEY_BOSS_ISSUE_HOST HEY_BOSS_ISSUE_PROJECT
cd "$root"
"$cli" fleet companion >owner.log 2>&1 &
owner=$!
trap 'kill "$owner" 2>/dev/null || true; wait "$owner" 2>/dev/null || true; rm -rf "$root"; rm -f "$socket.sock" "$socket.lock" "$socket.startup" "$socket.log"' EXIT
for ((i=0; i<200; i++)); do [ ! -S "$socket.sock" ] || break; sleep .05; done
test -S "$socket.sock"
"$cli" project init --project SpokenReview --prs false --worktree false --yes --json >/dev/null
say -v Samantha -o voice.aiff "Please move this paragraph after the introduction."
ffmpeg -nostdin -loglevel error -i voice.aiff -ar 16000 -ac 1 -f s16le voice.pcm
say -v Alex -o second.aiff "Please add a date to the last example."
ffmpeg -nostdin -loglevel error -i second.aiff -ar 16000 -ac 1 -f s16le second.pcm
HEY_BOSS_REVIEW_AUDIT="$audit" cargo test --manifest-path "$scripts/../Cargo.toml" -p hey-boss --test spoken_review -- --ignored --nocapture --test-threads=1
