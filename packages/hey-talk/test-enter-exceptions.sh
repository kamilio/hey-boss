#!/bin/bash
# Exercise the installed CLI and persistence. Run while idle: this restarts the app.
set -euo pipefail
cli="${HEY_TALK_TEST_CLI:-$HOME/.local/bin/hey-talk}"
state=$("$cli" status --json | jq -r .state)
case "$state" in recording|processing) printf 'Finish dictation before testing.\n' >&2; exit 1 ;; esac
fixture="local.hey-talk.test-$(uuidgen)"
cleanup() { "$cli" enter-exceptions remove "$fixture" --json >/dev/null; }
trap cleanup EXIT
"$cli" enter-exceptions add "$fixture" --json | jq -e --arg id "$fixture" '.ok and (.enter_exceptions | index($id) != null)' >/dev/null
"$cli" enter-exceptions add "$fixture" --json | jq -e --arg id "$fixture" '[.enter_exceptions[] | select(. == $id)] | length == 1' >/dev/null
"$cli" quit --json >/dev/null
sleep 0.3
"$cli" enter-exceptions --json | jq -e --arg id "$fixture" '.enter_exceptions | index($id) != null' >/dev/null
if "$cli" enter-exceptions add 'not a bundle id' >/dev/null 2>&1; then exit 1; fi
"$cli" enter-exceptions remove "$fixture" --json | jq -e --arg id "$fixture" '.enter_exceptions | index($id) == null' >/dev/null
printf 'PASS: CLI updates, duplicate handling, restart persistence, removal, invalid input\n'
