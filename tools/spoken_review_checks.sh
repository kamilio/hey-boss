#!/bin/bash
# Usage: tools/spoken_review_checks.sh /absolute/path/to/hey-boss /absolute/path/to/hey-boss-test
set -eu
scripts=$(cd "$(dirname "$0")" && pwd)
cli=$1
audit=$2
root=$(mktemp -d -t hb-spoken-review)
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
artifact() { "$cli" artifact --project named:SpokenReview --json "$@"; }
id=$(artifact create --title "Review fixture" --body $'# First heading\n\nFirst passage.\nSecond passage has several words and continues long enough to interrupt and resume reading.\n\nFinal passage.' | node -pe 'JSON.parse(require("fs").readFileSync(0)).artifact.id')
say -v Samantha -o voice.aiff "Please move this paragraph after the introduction."
ffmpeg -nostdin -loglevel error -i voice.aiff -ar 16000 -ac 1 -f s16le voice.pcm
node "$scripts/spoken_review_detection.mjs" "$cli" voice.pcm
HEY_BOSS_AUDIT_SPOKEN_REVIEW="$id" "$audit"
artifact view "$id" | node -e '
const {comments} = JSON.parse(require("fs").readFileSync(0));
require("assert").strictEqual(comments.length, 2);
const [c, recovered] = comments;
require("assert")(recovered.body.endsWith("Keep this quote.") && !recovered.outdated);
require("assert")(c.quote.startsWith("Second passage") && !c.outdated && c.body.startsWith("Line 4 · revision 1") && /move this paragraph after the introduction/i.test(c.body), JSON.stringify(c));
console.log("PASS saved transcript, source line, revision and web selection anchor");
'
