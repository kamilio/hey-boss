#!/bin/bash
# Exercises the native window with two synthetic microphone turns and real proxy inference.
set -eu
root=$(mktemp -d "$PWD/.conversation-test.XXXXXX")
trap 'rm -rf "$root"' EXIT
resources=$(dirname "$2")
if [ "$(basename "$resources")" = MacOS ]; then resources="$resources/../Resources"; fi
cargo run --quiet --manifest-path assets/conversation-native/Cargo.toml --example gallery -- "$resources" "$root"
HEY_BOSS_CLI_PATH="$1" HEY_BOSS_AUDIT_MEDIA="$root" "$2"
HEY_BOSS_AUDIT_PILL_ESCAPE="$root/escape" "$2"
say -v Samantha -o "$root/selection.aiff" 'Compare the apple. [[slnc 1600]] Now explain the orange.'
ffmpeg -nostdin -loglevel error -i "$root/selection.aiff" -ar 16000 -ac 1 -f s16le "$root/selection.pcm"
HEY_BOSS_AUDIT_SELECTION_CHAT="$root" "$2"
say -v Samantha -o "$root/first.aiff" 'Which fruit is on this page? Answer in one word.'
say -v Alex -o "$root/second.aiff" 'Which fruit did I mention earlier? Answer in one word.'
for turn in first second; do
  ffmpeg -nostdin -loglevel error -i "$root/$turn.aiff" -ar 16000 -ac 1 -f s16le "$root/$turn.pcm"
done
say -v Samantha -o "$root/question.aiff" 'What is an AWS region?'
ffmpeg -nostdin -loglevel error -i "$root/question.aiff" -ar 16000 -ac 1 -f s16le "$root/question.pcm"
# The reported cutoff: a short opener, a thinking pause, then the actual question.
say -v Samantha -o "$root/so.aiff" 'So'
ffmpeg -nostdin -loglevel error -i "$root/so.aiff" -ar 16000 -ac 1 -f s16le "$root/so.pcm"
HEY_BOSS_CLI_PATH="$1" HEY_BOSS_AUDIT_TURN_END="$root" "$2"
dd if=/dev/zero bs=51200 count=1 2>/dev/null >> "$root/so.pcm"
cat "$root/first.pcm" >> "$root/so.pcm"
mv "$root/so.pcm" "$root/first.pcm"
HEY_BOSS_CLI_PATH="$1" HEY_BOSS_AUDIT_CONVERSATION="$root" "$2"
say -v Samantha -o "$root/mobile.aiff" 'Only on mobile.'
say -v Alex -o "$root/accounts.aiff" 'Only after switching accounts.'
mkdir "$root/interruptions"
ffmpeg -nostdin -loglevel error -i "$root/mobile.aiff" -ar 16000 -ac 1 -f s16le "$root/interruptions/first.pcm"
ffmpeg -nostdin -loglevel error -i "$root/accounts.aiff" -ar 16000 -ac 1 -f s16le "$root/interruptions/second.pcm"
say -v Samantha -o "$root/ack.aiff" "Yeah."
ffmpeg -nostdin -loglevel error -i "$root/ack.aiff" -ar 16000 -ac 1 -f s16le "$root/interruptions/ack.pcm"
HEY_BOSS_CLI_PATH="$1" HEY_BOSS_AUDIT_INTERRUPTED_CHAT="$root/interruptions" "$2"
say -v Samantha -o "$root/reply.aiff" 'Here is more of the conversation. The next example needs a longer explanation.'
say -v Alex -o "$root/blue.aiff" 'Blue.'
for turn in reply blue; do
  ffmpeg -nostdin -loglevel error -i "$root/$turn.aiff" -ar 16000 -ac 1 -f s16le "$root/$turn.pcm"
done
HEY_BOSS_CLI_PATH="$1" HEY_BOSS_AUDIT_HEADPHONE_INTERRUPT="$root" "$2"
HEY_BOSS_CLI_PATH="$1" HEY_BOSS_AUDIT_FOREGROUND_MIC="$root" "$2"
HEY_BOSS_AUDIT_DICTATION_HANDOFF="$root/handoff" "$2"
HEY_BOSS_HANDOFF_TEST_BINARY="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")" swift test --package-path packages/hey-talk --filter testDictationReturnsMicrophoneToListeningAndSpeakingConversation
