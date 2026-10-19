#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
stage=$(mktemp -d "${TMPDIR:-/tmp}/hey-boss-signing-test.XXXXXX")
trap 'rm -rf "$stage"' EXIT
app="$stage/Hey Boss.app"
swift package_hey_boss.swift /usr/bin/true "$app"
requirement=$(codesign -dr - "$app" 2>&1 | sed -n 's/^designated => //p')
case "$requirement" in
  *'certificate leaf = H'*) ;;
  *) printf 'FAIL: desktop app lacks a persistent signing identity\n' >&2; exit 1 ;;
esac
old_hash=$(codesign -dvvv "$app" 2>&1 | sed -n 's/^CDHash=//p')
swift package_hey_boss.swift /usr/bin/false "$app"
new_requirement=$(codesign -dr - "$app" 2>&1 | sed -n 's/^designated => //p')
new_hash=$(codesign -dvvv "$app" 2>&1 | sed -n 's/^CDHash=//p')
test -n "$old_hash" && test -n "$new_hash"
test "$old_hash" != "$new_hash"
test "$requirement" = "$new_requirement"
codesign --verify --deep --strict -R "=$requirement" "$app"
codesign --force --sign - "$app"
if codesign --verify --strict -R "=$requirement" "$app" 2>/dev/null; then
  printf 'FAIL: ad-hoc replacement satisfied the certificate requirement\n' >&2
  exit 1
fi
printf 'PASS: changed desktop builds retain identity; ad-hoc replacements are rejected\n'
