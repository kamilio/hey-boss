#!/bin/bash
# Exercise the packaged worker with synthetic speech, without touching the app,
# microphone, clipboard, preferences, or dictation history.
set -euo pipefail
cd "$(dirname "$0")"
export HEY_TALK_TEST_APP="${HEY_TALK_TEST_APP:-$PWD/dist/Hey Talk.app}"
swift test --filter InferenceVocabularyTests
