#!/bin/bash
# Restore the existing identity, never generate a replacement on routine builds.
set -euo pipefail
cd "$(dirname "$0")"
umask 077
identity=$(tr -d '[:space:]' < signing-identity.sha1)
if security find-identity -p codesigning | grep -Fq "$identity"; then
  printf 'Persistent signing identity is already installed.\n'
  exit 0
fi
stage=$(mktemp -d "${TMPDIR:-/tmp}/hey-talk-signing-restore.XXXXXX")
trap 'rm -rf "$stage"' EXIT
item='op://Private/Hey Talk local code signing'
op read "$item/identity" --out-file "$stage/identity.p12" >/dev/null
op read "$item/password" --out-file "$stage/password" >/dev/null
actual=$(openssl pkcs12 -in "$stage/identity.p12" -passin "file:$stage/password" -clcerts -nokeys | openssl x509 -noout -fingerprint -sha1 | sed 's/.*=//;s/://g')
if [ "$actual" != "$identity" ]; then
  printf 'The 1Password certificate does not match the pinned signing identity.\n' >&2
  exit 1
fi
keychain=$(security default-keychain -d user | sed 's/^[[:space:]]*"//;s/"$//')
security import "$stage/identity.p12" -k "$keychain" -P "$(cat "$stage/password")" -T /usr/bin/codesign
printf 'Persistent signing identity restored. macOS may request access to its key on first use.\n'
