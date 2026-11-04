#!/bin/bash
# Exercises the native window with two synthetic microphone turns and real proxy inference.
set -eu
root=$(mktemp -d "$PWD/.conversation-test.XXXXXX")
trap 'rm -rf "$root"' EXIT
say -v Samantha -o "$root/first.aiff" 'Which fruit is on this page? Answer in one word.'
say -v Alex -o "$root/second.aiff" 'Which fruit did I mention earlier? Answer in one word.'
for turn in first second; do
  ffmpeg -nostdin -loglevel error -i "$root/$turn.aiff" -ar 16000 -ac 1 -f s16le "$root/$turn.pcm"
done
# The reported cutoff: a short opener, a thinking pause, then the actual question.
say -v Samantha -o "$root/so.aiff" 'So'
ffmpeg -nostdin -loglevel error -i "$root/so.aiff" -ar 16000 -ac 1 -f s16le "$root/so.pcm"
dd if=/dev/zero bs=51200 count=1 2>/dev/null >> "$root/so.pcm"
cat "$root/first.pcm" >> "$root/so.pcm"
mv "$root/so.pcm" "$root/first.pcm"
HEY_BOSS_CLI_PATH="$1" HEY_BOSS_AUDIT_CONVERSATION="$root" "$2"
