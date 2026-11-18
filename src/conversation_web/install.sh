#!/bin/bash
# Set up a paired HTTPS Home Screen app on this Mac's local network.
set -euo pipefail
umask 077
binary=$1
state=$(cat "$(dirname "$binary")/hey-boss.state")
web="$state/conversation-web"
host="$(scutil --get LocalHostName).local"
mkdir -p "$web"
if ! test -s "$web/server.pem" || ! test -s "$web/server.key" || ! test -s "$web/Hey Boss Wi-Fi.cer" || ! grep -Fqx "$host:4785" "$web/host" || ! openssl x509 -in "$web/server.pem" -checkend 86400 -noout >/dev/null; then
  stage=$(mktemp -d "$web/setup.XXXXXX")
  trap 'rm -rf "$stage"' EXIT
  # Only this leaf can be issued: destroy the CA signing key after setup.
  cat >"$stage/extensions" <<EXT
[req]
distinguished_name = dn
[dn]
[ca]
basicConstraints=critical,CA:TRUE,pathlen:0
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
[server]
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:$host
EXT
  openssl req -new -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 -subj "/CN=Hey Boss Wi-Fi ($host)" -config "$stage/extensions" -extensions ca -keyout "$stage/ca.key" -out "$stage/ca.pem" 2>/dev/null
  openssl req -new -newkey rsa:2048 -nodes -subj "/CN=$host" -keyout "$stage/server.key" -out "$stage/request" 2>/dev/null
  openssl x509 -req -sha256 -days 180 -in "$stage/request" -CA "$stage/ca.pem" -CAkey "$stage/ca.key" -set_serial "0x$(openssl rand -hex 16)" -extfile "$stage/extensions" -extensions server -out "$stage/server.pem"
  openssl x509 -in "$stage/ca.pem" -outform der -out "$web/Hey Boss Wi-Fi.cer"
  mv "$stage/server.key" "$stage/server.pem" "$web/"
  printf '%s:4785\n' "$host" > "$web/host"
fi
# Trust only this server certificate locally. Chrome rejects hostname-constrained CA trust.
security add-trusted-cert -r trustAsRoot -p ssl -k "$HOME/Library/Keychains/login.keychain-db" "$web/server.pem"
test -s "$web/pairing-code" || openssl rand -hex 16 >"$web/pairing-code"
label=local.hey-boss.conversation-web
plist="$HOME/Library/LaunchAgents/$label.plist"
mkdir -p "$(dirname "$plist")"
plutil -create xml1 "$web/service.plist"
plutil -insert Label -string "$label" "$web/service.plist"
plutil -insert ProgramArguments -json '[]' "$web/service.plist"
i=0
for arg in "$binary" agent chats web; do
  plutil -insert "ProgramArguments.$i" -string "$arg" "$web/service.plist"; i=$((i+1))
done
plutil -insert KeepAlive -bool true "$web/service.plist"
plutil -insert StandardErrorPath -string "$web/service.log" "$web/service.plist"
mv "$web/service.plist" "$plist"
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$plist"
echo "Safari: https://$host:4785/"
echo "AirDrop certificate: $web/Hey Boss Wi-Fi.cer"
echo "Pairing code: hey-boss agent chats web --pairing-code"
