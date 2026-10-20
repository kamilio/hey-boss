#!/bin/bash
# Hardware regression: switch the system input during capture, then restore it.
# All captured audio is discarded; no paste, transcription, or history writes.
set -euo pipefail
cd "$(dirname "$0")"
cli="${HEY_TALK_TEST_CLI:-$HOME/.local/bin/hey-talk}"
status=$("$cli" status --json)
printf '%s' "$status" | jq -e '.microphone == "granted" and .input == "default" and (.state == "idle" or .state == "failed")' >/dev/null || {
  printf 'Use the default input, grant microphone access, and finish dictation before this test.\n' >&2
  exit 1
}
stage=$(mktemp -d "${TMPDIR:-/tmp}/hey-talk-input-test.XXXXXX")
cat > "$stage/route.swift" <<'SWIFT'
import Foundation
import CoreAudio
let system = AudioObjectID(kAudioObjectSystemObject)
func uid(_ device: AudioDeviceID) -> String? {
    var address = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyDeviceUID, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var value: Unmanaged<CFString>?, size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, &value) == noErr else { return nil }
    return value?.takeUnretainedValue() as String?
}
var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDefaultInputDevice, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
var device: AudioDeviceID = 0, size = UInt32(MemoryLayout<AudioDeviceID>.size)
guard AudioObjectGetPropertyData(system, &address, 0, nil, &size, &device) == noErr else { exit(1) }
if CommandLine.arguments.count == 1 {
    guard let value = uid(device) else { exit(1) }; print(value)
} else {
    var listAddress = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var listSize: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(system, &listAddress, 0, nil, &listSize) == noErr else { exit(1) }
    var ids = [AudioDeviceID](repeating: 0, count: Int(listSize) / MemoryLayout<AudioDeviceID>.size)
    guard AudioObjectGetPropertyData(system, &listAddress, 0, nil, &listSize, &ids) == noErr,
          let target = ids.first(where: { uid($0) == CommandLine.arguments[1] }) else { exit(1) }
    device = target
    guard AudioObjectSetPropertyData(system, &address, 0, nil, size, &device) == noErr else { exit(1) }
}
SWIFT
trap 'rm -rf "$stage"' EXIT
swiftc "$stage/route.swift" -o "$stage/route"
original=$("$stage/route")
alternate=$("$cli" devices --json | jq -r --arg original "$original" '
  [.devices[] | select(.uid != $original) | select(.uid == "BuiltInMicrophoneDevice" or (.uid | endswith(":input")) or (.uid | startswith("AppleUSBAudioEngine:")))]
  | sort_by(if .uid == "BuiltInMicrophoneDevice" then 0 else 1 end) | .[0].uid // empty')
if [ -z "$alternate" ]; then printf 'Connect a second microphone for the switching test.\n' >&2; exit 1; fi
cleanup() {
  "$cli" cancel --json >/dev/null || true
  "$stage/route" "$original" || printf 'Could not restore the original system input.\n' >&2
  rm -rf "$stage"
}
trap cleanup EXIT
before=$("$cli" history --json | jq -c '[.entries[].id]')
if ! "$cli" start --no-paste --json > "$stage/start.json"; then cat "$stage/start.json" >&2; exit 1; fi
previous=0
for iteration in {1..5}; do
  for input in "$alternate" "$original"; do
    "$stage/route" "$input"
    matched=false
    routed=false
    for attempt in {1..80}; do
      status=$("$cli" status --json)
      if printf '%s' "$status" | jq -e --arg input "$input" --argjson previous "$previous" '
        .state == "recording" and .active_input.uid == $input and .sample_count >= $previous' >/dev/null; then
        if [ "$routed" = false ]; then
          # Count fresh buffers after the new route is selected. The alternate
          # may be silent (for example, the built-in mic with the lid closed).
          previous=$(printf '%s' "$status" | jq .sample_count)
          routed=true
        elif printf '%s' "$status" | jq -e --arg input "$input" --arg original "$original" --argjson previous "$previous" '
          .sample_count > $previous and ($input != $original or .microphone_ready)' >/dev/null; then
          matched=true; break
        fi
      fi
      sleep 0.1
    done
    if [ "$matched" != true ]; then printf 'Input did not recover: %s\n%s\n' "$input" "$status" >&2; exit 1; fi
    previous=$(printf '%s' "$status" | jq .sample_count)
    printf '%s' "$status" | jq '{input: .active_input.name, samples: .sample_count, restarts: .capture_restarts, microphone_ready, rms}'
  done
done
"$cli" cancel --json >/dev/null
test "$("$cli" history --json | jq -c '[.entries[].id]')" = "$before"
printf 'PASS: 10 live system-input changes, accumulated audio retained, no history writes\n'
