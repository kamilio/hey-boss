#!/bin/bash
# Runs against the installed Hey Talk; requires Accessibility and Screen Recording.
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
node --input-type=module <<'JS'
import assert from 'node:assert/strict';
import {createConnection} from 'node:net';
import {homedir} from 'node:os';
async function inspect(mode, target = 'WindowCaptureFixture') {
  const socket = createConnection(homedir() + '/Library/Application Support/hey-talk/control.sock');
  socket.write(JSON.stringify({command:'inspect_window', target, mode}) + '\n');
  const chunks = [];
  for await (const chunk of socket) chunks.push(chunk);
  return JSON.parse(Buffer.concat(chunks).toString());
}
const ax = await inspect('accessibility');
assert.ok(ax.ok, ax.error); assert.match(ax.text, /Marigold capture test/);
assert.ok(!ax.jpeg && !ax.text.includes('must-not-appear'));
assert.equal((await inspect('both', 'No Such Window Fixture')).ok, false);
console.log('PASS named window, accessibility text, secure-field omission, and no fallback to an unrelated app');
for (const mode of ['screenshot','both']) {
  const result = await inspect(mode);
  assert.ok(result.ok, result.error);
  assert.ok(Buffer.from(result.jpeg, 'base64').subarray(0, 2).equals(Buffer.from([255,216])));
  if (mode === 'both') assert.match(result.text, /Marigold capture test/); else assert.equal(result.text, undefined);
}
console.log('PASS screenshot alone and with accessibility from the same window');
JS
