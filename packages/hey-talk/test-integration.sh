#!/bin/bash
# Exercises the packaged native processes using synthetic speech, no mic or paste.
set -euo pipefail
cd "$(dirname "$0")"
cli="$PWD/dist/Hey Talk.app/Contents/MacOS/hey-talk"
fixture="$PWD/.build/test-speech.aiff"
say -v Samantha -o "$fixture" 'Hey Talk records my voice and turns it into text. Everything runs locally on this Mac.'
"$cli" warm --json > .build/warm.json
service_pid=$("$cli" status --json | jq -r .pid)
for iteration in 1 2 3; do
  "$cli" transcribe "$fixture" --json | tee ".build/inference-$iteration.json"
  jq -e '.ok and (.text | ascii_downcase | contains("everything runs locally on this mac")) and .delivery == "stdout" and .real_time_factor < 1' ".build/inference-$iteration.json" >/dev/null
done
worker_pid=$("$cli" status --json | jq -r .inference_pid)
kill -KILL "$worker_pid"
"$cli" transcribe "$fixture" --json > .build/recovered.json
jq -e '.ok and (.text | length > 20)' .build/recovered.json >/dev/null
test "$("$cli" status --json | jq -r .pid)" = "$service_pid"
test "$("$cli" status --json | jq -r .inference_pid)" != "$worker_pid"
# Cancel while file decoding/inference is in flight. The old result must not win.
"$cli" transcribe "$fixture" --json > .build/cancelled.json &
client=$!
for attempt in {1..40}; do
  if [ "$("$cli" status --json | jq -r .state)" = processing ]; then break; fi
  sleep 0.025
done
"$cli" cancel --json > .build/cancel.json
if wait "$client"; then printf 'Cancelled transcription unexpectedly succeeded\n' >&2; exit 1; fi
jq -e '.ok == false and .error == "Cancelled"' .build/cancelled.json >/dev/null
test "$("$cli" status --json | jq -r .state)" = idle
"$cli" transcribe "$fixture" --json > .build/after-cancel.json
jq -e '.ok' .build/after-cancel.json >/dev/null
"$cli" devices --json | jq -e '.ok and (.devices | length > 0)' >/dev/null
test "$(stat -f %Lp "$HOME/Library/Application Support/hey-talk/control.sock")" = 600
printf 'PASS: native transcription, faster-than-realtime decode, worker crash recovery, cancellation, input enumeration, private socket\n'
