#!/bin/bash
# Runs against the installed Hey Talk; requires Accessibility; also checks Screen Recording denial.
set -eu
cd "$(dirname "$0")"
mkdir -p .build
stage=$(mktemp -d "$PWD/.build/window-check.XXXXXX")
pid=''
trap 'if [[ -n "$pid" ]]; then kill "$pid" 2>/dev/null || true; fi; rm -rf "$stage"' EXIT
cd "$stage"
swiftc -o WindowCaptureFixture - <<'SWIFT'
import AppKit
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 420, height: 140), styleMask: [.titled], backing: .buffered, defer: false)
window.title = "Window capture fixture"
let text = NSTextField(labelWithString: "Marigold capture test")
text.frame = NSRect(x: 20, y: 80, width: 350, height: 28); window.contentView!.addSubview(text)
let secret = NSSecureTextField(frame: NSRect(x: 20, y: 30, width: 250, height: 24))
secret.stringValue = "must-not-appear"; window.contentView!.addSubview(secret)
window.center(); window.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
FileManager.default.createFile(atPath: "ready", contents: nil)
app.run()
SWIFT
./WindowCaptureFixture &
pid=$!
for attempt in {1..100}; do [[ -e ready ]] && break; sleep 0.05; done
test -e ready
cargo run --quiet --locked -p hey-boss --example inspect-hey-talk-window
