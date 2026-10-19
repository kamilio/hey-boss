#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
app="${1:?Usage: sign-app.sh APP_PATH}"
identity=$(tr -d '[:space:]' < signing-identity.sha1)
if ! security find-identity -p codesigning | grep -Fq "$identity"; then
  printf 'Restore the persistent signing identity with ./setup-signing.sh before building.\n' >&2
  exit 1
fi
# Inference has no microphone or Accessibility privileges. Only the desktop
# app needs the persistent identity; do not silently fall back to ad-hoc signing.
codesign --force --sign "$identity" --timestamp=none --options runtime --entitlements entitlements.plist "$app"
codesign --verify --deep --strict -R "=identifier \"local.hey-talk\" and certificate leaf = H\"$identity\"" "$app"
