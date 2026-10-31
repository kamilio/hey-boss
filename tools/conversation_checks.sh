#!/bin/bash
# Exercises the native window with two synthetic microphone turns and real on-device inference.
set -eu
root=$(mktemp -d -t hb-conversation)
trap 'rm -rf "$root"' EXIT
say -v Samantha -o "$root/first.aiff" 'My favorite fruit is a pineapple.'
say -v Alex -o "$root/second.aiff" 'Which fruit did I mention earlier? Answer in one word.'
for turn in first second; do
  ffmpeg -nostdin -loglevel error -i "$root/$turn.aiff" -ar 16000 -ac 1 -f s16le "$root/$turn.pcm"
done
HEY_BOSS_CLI_PATH="$1" HEY_BOSS_AUDIT_CONVERSATION="$root" "$2"
