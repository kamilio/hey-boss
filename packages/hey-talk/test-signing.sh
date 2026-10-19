#!/bin/bash
# Verify identity survives changed code/resources, and cannot be copied by ad-hoc signing.
set -euo pipefail
cd "$(dirname "$0")"
stage=$(mktemp -d "${TMPDIR:-/tmp}/hey-talk-signing-test.XXXXXX")
trap 'rm -rf "$stage"' EXIT
app="$stage/Hey Talk.app"
ditto 'dist/Hey Talk.app' "$app"
requirement=$(codesign -dr - "$app" 2>&1 | sed -n 's/^designated => //p')
case "$requirement" in
  *'certificate leaf = H'*) ;;
  *) printf 'Build with the persistent certificate first.\n' >&2; exit 1 ;;
esac
old_hash=$(codesign -dvvv "$app" 2>&1 | sed -n 's/^CDHash=//p')
test -n "$old_hash"
# Different executable, same bundle identifier: this must remain the same app identity.
cp /usr/bin/true "$app/Contents/MacOS/hey-talk"
printf 'changed resource\n' > "$app/Contents/Resources/signing-test.txt"
./sign-app.sh "$app"
new_requirement=$(codesign -dr - "$app" 2>&1 | sed -n 's/^designated => //p')
new_hash=$(codesign -dvvv "$app" 2>&1 | sed -n 's/^CDHash=//p')
test -n "$new_hash"
test "$old_hash" != "$new_hash"
test "$requirement" = "$new_requirement"
codesign --verify --deep --strict -R "=$requirement" "$app"
codesign --force --sign - "$app"
if codesign --verify --strict -R "=$requirement" "$app" 2>/dev/null; then
  printf 'FAIL: ad-hoc replacement satisfied the certificate requirement\n' >&2
  exit 1
fi
printf 'PASS: changed builds retain identity; ad-hoc replacements are rejected\n'
