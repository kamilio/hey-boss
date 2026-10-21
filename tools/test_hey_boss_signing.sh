#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
stage=$(mktemp -d "${TMPDIR:-/tmp}/hey-boss-signing-test.XXXXXX")
trap 'rm -rf "$stage"' EXIT
app="$stage/Hey Boss.app"
# Cargo's linker signature follows the executable hash. Both a changed build and
# a differently named bootstrap copy must retain the installed CLI's identity.
cli="$stage/hey-boss"
cp /usr/bin/true "$cli"
swift package_hey_boss.swift --cli "$cli"
cli_requirement=$(codesign -dr - "$cli" 2>&1 | sed -n 's/^designated => //p')
case "$cli_requirement" in
  *'identifier "local.hey-boss.cli"'*'certificate leaf = H'*) ;;
  *) printf 'FAIL: CLI lacks a stable identifier and certificate\n' >&2; exit 1 ;;
esac
cli_hash=$(codesign -dvvv "$cli" 2>&1 | sed -n 's/^CDHash=//p')
cp /usr/bin/false "$stage/guard"
swift package_hey_boss.swift --cli "$stage/guard"
test "$cli_requirement" = "$(codesign -dr - "$stage/guard" 2>&1 | sed -n 's/^designated => //p')"
test "$cli_hash" != "$(codesign -dvvv "$stage/guard" 2>&1 | sed -n 's/^CDHash=//p')"
mv "$stage/guard" "$cli"
codesign --verify --strict -R "=$cli_requirement" "$cli"
codesign --force --sign - "$cli"
if codesign --verify --strict -R "=$cli_requirement" "$cli" 2>/dev/null; then
  printf 'FAIL: ad-hoc CLI replacement satisfied the certificate requirement\n' >&2
  exit 1
fi
printf 'PASS: CLI rebuilds and bootstrap copies retain identity; ad-hoc replacements are rejected\n'
# An unavailable pinned certificate must fail before altering the input, never
# silently fall back to Cargo's ad-hoc signature.
mkdir -p "$stage/missing/assets"
cp package_hey_boss.swift "$stage/missing/"
printf '%040d\n' 0 > "$stage/missing/assets/signing-identity.sha1"
before=$(shasum -a 256 "$cli")
if swift "$stage/missing/package_hey_boss.swift" --cli "$cli" > "$stage/missing.log" 2>&1; then
  printf 'FAIL: missing pinned certificate was accepted\n' >&2
  exit 1
fi
test "$before" = "$(shasum -a 256 "$cli")"
printf 'PASS: missing certificate leaves the input unchanged\n'
swift package_hey_boss.swift /usr/bin/true "$app"
# CLI-only changes must not require another write to a protected app bundle.
swift package_hey_boss.swift --check-desktop "$app"
mkdir -p "$stage/source/assets" "$stage/source/src"
cp package_hey_boss.swift hey_boss_daemon.swift Cargo.toml "$stage/source/"
cp assets/hey-boss.icns assets/signing-identity.sha1 "$stage/source/assets/"
printf '// Changed CLI source\n' > "$stage/source/src/main.rs"
swift "$stage/source/package_hey_boss.swift" --check-desktop "$app"
printf '\n// Changed desktop source\n' >> "$stage/source/hey_boss_daemon.swift"
if swift "$stage/source/package_hey_boss.swift" --check-desktop "$app"; then
  printf 'FAIL: changed desktop source was treated as installed\n' >&2
  exit 1
else
  test "$?" = 3
fi
printf 'PASS: CLI-only changes preserve the desktop; changed desktop inputs require replacement\n'
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
if swift package_hey_boss.swift --check-desktop "$app"; then
  printf 'FAIL: ad-hoc app was treated as current\n' >&2
  exit 1
else
  test "$?" = 3
fi
if codesign --verify --strict -R "=$requirement" "$app" 2>/dev/null; then
  printf 'FAIL: ad-hoc replacement satisfied the certificate requirement\n' >&2
  exit 1
fi
printf 'PASS: changed desktop builds retain identity; ad-hoc replacements are rejected\n'
