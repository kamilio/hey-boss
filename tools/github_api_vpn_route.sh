#!/bin/sh
set -eu

# Repair the missing GitHub API route only when called after an IP rejection.
# Follow the current VPN gateway instead of pinning an interface across reconnects.
reference=$(/sbin/route -n get 140.82.112.5 2>/dev/null) || exit 1
interface=$(printf '%s\n' "$reference" | /usr/bin/awk '/interface:/ {print $2}')
gateway=$(printf '%s\n' "$reference" | /usr/bin/awk '/gateway:/ {print $2}')
case "$interface" in utun[0-9]*) ;; *) exit 1 ;; esac
[ -n "$gateway" ] || exit 1

current=$(/sbin/route -n get 140.82.112.6 2>/dev/null) || exit 1
current_interface=$(printf '%s\n' "$current" | /usr/bin/awk '/interface:/ {print $2}')
current_gateway=$(printf '%s\n' "$current" | /usr/bin/awk '/gateway:/ {print $2}')
if [ "$current_interface" = "$interface" ] && [ "$current_gateway" = "$gateway" ]; then
    exit 0
fi

/sbin/route -n add -host 140.82.112.6 "$gateway" 2>/dev/null ||
    /sbin/route -n change -host 140.82.112.6 "$gateway"
