#!/bin/bash
# Exercises the packaged native processes using synthetic speech, no mic or paste.
set -euo pipefail
cd "$(dirname "$0")"
cli="$PWD/dist/Hey Talk.app/Contents/MacOS/hey-talk"
history_db="$HOME/Library/Application Support/hey-talk/history.sqlite3"
history_ids="$PWD/.build/test-history-ids"
: > "$history_ids"
remember_history() { jq -er '.history_id' "$1" >> "$history_ids"; }
cleanup_history() {
  while IFS= read -r id; do
    if [[ "$id" =~ ^[A-Fa-f0-9-]{36}$ ]]; then
      sqlite3 "$history_db" "DELETE FROM dictations WHERE id = '$id';"
    fi
  done < "$history_ids"
}
trap cleanup_history EXIT
fixture="$PWD/.build/test-speech.aiff"
say -v Samantha -o "$fixture" 'Hey Talk records my voice and turns it into text. Everything runs locally on this Mac.'
long_fixture="$PWD/.build/test-long-speech.aiff"
long_speech=''
for iteration in {1..20}; do long_speech+='This longer recording checks that cancellation discards the transcript while decoding is still running. '; done
say -v Samantha -o "$long_fixture" "$long_speech"
"$cli" warm --json > .build/warm.json
service_pid=$("$cli" status --json | jq -r .pid)
for iteration in 1 2 3; do
  "$cli" transcribe "$fixture" --json | tee ".build/inference-$iteration.json"
  remember_history ".build/inference-$iteration.json"
  jq -e '.ok and .history_saved and (.text | ascii_downcase | contains("everything runs locally on this mac")) and .delivery == "stdout" and .real_time_factor < 1' ".build/inference-$iteration.json" >/dev/null
done
worker_pid=$("$cli" status --json | jq -r .inference_pid)
kill -KILL "$worker_pid"
"$cli" transcribe "$fixture" --json > .build/recovered.json
remember_history .build/recovered.json
jq -e '.ok and (.text | length > 20)' .build/recovered.json >/dev/null
test "$("$cli" status --json | jq -r .pid)" = "$service_pid"
test "$("$cli" status --json | jq -r .inference_pid)" != "$worker_pid"
# Cancel while file decoding/inference is in flight. The old result must not win.
"$cli" history --json > .build/history-before-cancel.json
"$cli" transcribe "$long_fixture" --json > .build/cancelled.json &
client=$!
for attempt in {1..40}; do
  if [ "$("$cli" status --json | jq -r .state)" = processing ]; then break; fi
  sleep 0.025
done
"$cli" cancel --json > .build/cancel.json
if wait "$client"; then remember_history .build/cancelled.json; printf 'Cancelled transcription unexpectedly succeeded\n' >&2; exit 1; fi
jq -e '.ok == false and .error == "Cancelled"' .build/cancelled.json >/dev/null
test "$("$cli" status --json | jq -r .state)" = idle
"$cli" history --json > .build/history-after-cancel.json
cmp .build/history-before-cancel.json .build/history-after-cancel.json
"$cli" transcribe "$fixture" --json > .build/after-cancel.json
remember_history .build/after-cancel.json
jq -e '.ok' .build/after-cancel.json >/dev/null
"$cli" devices --json | jq -e '.ok and (.devices | length > 0)' >/dev/null
test "$(stat -f %Lp "$HOME/Library/Application Support/hey-talk/control.sock")" = 600
test "$(stat -f %Lp "$history_db")" = 600
"$cli" quit --json >/dev/null
for attempt in {1..80}; do
  if ! kill -0 "$service_pid" 2>/dev/null; then break; fi
  sleep 0.025
done
"$cli" last --json > .build/last-restarted.json
jq -e --slurpfile last .build/after-cancel.json '.ok and .text == $last[0].text' .build/last-restarted.json >/dev/null
"$cli" history --json > .build/history-restarted.json
jq -e --slurpfile last .build/after-cancel.json '.ok and .entries[0].id == $last[0].history_id and .entries[0].text == $last[0].text' .build/history-restarted.json >/dev/null
"$cli" history --before "$(jq -r .history_id .build/after-cancel.json)" --json | jq -e --slurpfile previous .build/recovered.json '.entries[0].id == $previous[0].history_id' >/dev/null
"$cli" status --json | jq -e '.hotkey_registered and .hotkey == "Control+Command+Option+Shift+R"' >/dev/null
printf 'PASS: native transcription, faster-than-realtime decode, worker crash recovery, cancellation, input enumeration, private socket, durable history, Hyper+R registration\n'
