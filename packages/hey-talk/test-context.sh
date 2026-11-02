#!/bin/bash
# Hardware test: the selected mic must be able to hear speech from the speakers.
set -euo pipefail
cd "$(dirname "$0")"
cli="${HEY_TALK_TEST_CLI:-$HOME/.local/bin/hey-talk}"
"$cli" status --json | jq -e '.state == "idle" and (.waiting_to_paste|not) and .accessibility and .correction.enabled and (.auto.enabled|not)' >/dev/null || {
  echo 'Run while idle, with Accessibility and AI Correction on and Auto off.' >&2; exit 1
}
mkdir -p .build
work=$(mktemp -d "$PWD/.build/context-test.XXXXXX")
recording=false
fixture_pid=''
cleanup() {
  if $recording; then "$cli" cancel --json >/dev/null || true; fi
  if [[ -n "$fixture_pid" ]]; then kill "$fixture_pid" 2>/dev/null || true; wait "$fixture_pid" 2>/dev/null || true; fi
  local id
  if id=$(jq -er '.history_id // empty' "$work/result" 2>/dev/null) && [[ "$id" =~ ^[A-Fa-f0-9-]{36}$ ]]; then
    sqlite3 "$HOME/Library/Application Support/hey-talk/history.sqlite3" "DELETE FROM dictations WHERE id = '$id';"
  fi
  rm -rf "$work"
}
trap cleanup EXIT
cd "$work"
swiftc -o fixture - <<'SWIFT'
import AppKit
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 420, height: 120), styleMask: [.titled], backing: .buffered, defer: false)
let label = NSTextField(labelWithString: "Marigold starting page")
label.frame = NSRect(x: 20, y: 40, width: 380, height: 30)
window.contentView!.addSubview(label)
window.center()
window.makeKeyAndOrderFront(nil)
signal(SIGUSR1, SIG_IGN)
let change = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
change.setEventHandler { label.stringValue = "Cobalt ending page" }
change.resume()
NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: app, queue: .main) { _ in
    FileManager.default.createFile(atPath: "ready", contents: nil)
}
app.activate(ignoringOtherApps: true)
app.run()
SWIFT
./fixture &
fixture_pid=$!
for attempt in {1..100}; do [[ -e ready ]] && break; sleep 0.05; done
test -e ready
"$cli" start --no-paste --json | jq -e '.state == "recording"' >/dev/null
recording=true
say 'Checking the dictation context snapshot.'
kill -USR1 "$fixture_pid"
sleep 0.3
"$cli" stop --json > result
recording=false
jq -e '.history_saved and .delivery == "stdout" and (.app_context | contains("Marigold starting page") and (contains("Cobalt ending page")|not))' result >/dev/null || {
  echo 'History did not retain the starting page context.' >&2; exit 1
}
echo 'PASS: correction kept the text visible at recording start.'
