#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
stage=$(mktemp -d "$PWD/.ssh-signing-test.XXXXXX")
trap 'rm -rf "$stage"' EXIT
identity=$(tr -d '[:space:]' < assets/signing-identity.sha1)
# SSH sessions do not share the desktop's unlocked login keychain.
export SSH_CONNECTION="${SSH_CONNECTION:-local-signing-test}"
binary="$stage/guard with 'spaces'"
cp /usr/bin/true "$binary"
swift package_hey_boss.swift --cli "$binary"
codesign --verify --strict -R "=identifier \"local.hey-boss.cli\" and certificate leaf = H\"$identity\"" "$binary"
if swift package_hey_boss.swift --cli "$stage/missing" >"$stage/failure.log" 2>&1; then
  echo 'FAIL: remote signing swallowed a codesign failure' >&2
  exit 1
fi
if find "$stage" -name '.signing-*' | grep -q .; then
  echo 'FAIL: remote signing left staging files behind' >&2
  exit 1
fi
echo 'PASS: SSH signing uses the desktop keychain, handles spaced paths, propagates failure, and cleans up'
