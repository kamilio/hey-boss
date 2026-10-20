#!/bin/bash
# Explicit hardware test: captures the selected mic, discards all audio, never pastes.
set -euo pipefail
cd "$(dirname "$0")"
cli="${HEY_TALK_TEST_CLI:-$HOME/.local/bin/hey-talk}"
status=$("$cli" status --json)
printf '%s' "$status" | jq -e '.microphone == "granted" and (.state == "idle" or .state == "failed")' >/dev/null || {
  printf 'Grant Microphone permission and finish any dictation before this test.\n' >&2
  exit 1
}
trap '"$cli" cancel --json >/dev/null || true' EXIT
before=$("$cli" history --json | jq -c '[.entries[].id]')
for iteration in {1..10}; do
  "$cli" toggle --no-paste --json > ".build/microphone-start-$iteration.json"
  sleep 0.75
  "$cli" status --json > ".build/microphone-capture-$iteration.json"
  jq -e '.state == "recording" and .microphone_ready and .capture_restarts == 0 and .sample_count > 2400' ".build/microphone-capture-$iteration.json" >/dev/null
  "$cli" cancel --json | jq -e '.state == "idle" and (.microphone_ready | not)' >/dev/null
done
"$cli" toggle --no-paste --json > .build/microphone-sustained-start.json
for second in {1..30}; do
  sleep 1
  "$cli" status --json > .build/microphone-sustained.json
  jq -e --argjson second "$second" '.state == "recording" and .microphone_ready and .capture_restarts == 0 and .sample_count > ($second * 12000)' .build/microphone-sustained.json >/dev/null
done
"$cli" cancel --json >/dev/null
test "$("$cli" history --json | jq -c '[.entries[].id]')" = "$before"
jq -s '{captures: length, start_ms_min: (map(.capture_start_ms)|min), start_ms_max: (map(.capture_start_ms)|max), lowest_sample_count: (map(.sample_count)|min)}' .build/microphone-capture-{1..10}.json
jq '{sustained_samples: .sample_count, bluetooth: .bluetooth_input, peak_rms: .peak_rms}' .build/microphone-sustained.json
printf 'PASS: 10 capture/cancel cycles, 30-second continuous capture, no history writes\n'
