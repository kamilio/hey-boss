#!/bin/bash
# Exercise the installed, signed Hey Talk against two disposable native inputs.
set -eu
cd "$(dirname "$0")"
mkdir -p .build
stage=$(mktemp -d "$PWD/.build/conversation-paste.XXXXXX")
pid=''
trap 'if [[ -n "$pid" ]]; then kill "$pid" 2>/dev/null || true; fi; rm -rf "$stage"' EXIT
cd "$stage"
mkdir -p Fixture.app/Contents/MacOS
cat > Fixture.app/Contents/Info.plist <<'PLIST'
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Fixture</string><key>CFBundleIdentifier</key><string>local.hey-talk.paste-fixture</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>
PLIST
swiftc -o Fixture.app/Contents/MacOS/Fixture - <<'SWIFT'
import AppKit
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let menu = NSMenu(), edit = NSMenu(title: "Edit")
menu.addItem(withTitle: "Fixture", action: nil, keyEquivalent: "").submenu = NSMenu(title: "Fixture")
menu.addItem(withTitle: "Edit", action: nil, keyEquivalent: "").submenu = edit
edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
app.mainMenu = menu
func editor(_ name: String) -> (NSWindow, NSTextView) {
    let w = NSWindow(contentRect: NSRect(x: 60, y: 60, width: 400, height: 160), styleMask: [.titled], backing: .buffered, defer: false)
    w.title = name
    let text = NSTextView(frame: w.contentView!.bounds)
    w.contentView!.addSubview(text); w.makeFirstResponder(text)
    return (w, text)
}
let (first, original) = editor("Original window"), (second, other) = editor("Other window")
let password = NSSecureTextField(frame: NSRect(x: 10, y: 10, width: 250, height: 25))
password.stringValue = "private-password-marker"
second.contentView!.addSubview(password)
first.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in
    if FileManager.default.fileExists(atPath: "switch") {
        try! FileManager.default.removeItem(atPath: "switch")
        second.makeKeyAndOrderFront(nil); second.makeFirstResponder(other)
    }
    if FileManager.default.fileExists(atPath: "secure") {
        try! FileManager.default.removeItem(atPath: "secure")
        second.makeKeyAndOrderFront(nil); second.makeFirstResponder(password)
    }
    let state = [first.isKeyWindow ? "first" : second.isKeyWindow ? "second" : "inactive", original.string, other.string]
    try! JSONSerialization.data(withJSONObject: state).write(to: URL(fileURLWithPath: "state"), options: .atomic)
}
app.run()
SWIFT
Fixture.app/Contents/MacOS/Fixture &
pid=$!
ruby - <<'RUBY'
require 'socket'
require 'json'
def ask(command, **args)
  UNIXSocket.open(File.expand_path('~/Library/Application Support/hey-talk/control.sock')) do |s|
    s.puts(JSON.generate(args.merge(command: command)))
    JSON.parse(s.gets)
  end
end
def wait
  100.times { return if yield; sleep 0.05 }
  abort "Unexpected fixture state: #{File.read('state') rescue 'missing'}"
end
state = -> { JSON.parse(File.read('state')) }
wait { File.exist?('state') && state.call.first == 'first' }
ask('remember_focus')
context = ask('spoken_context')
abort context.inspect unless context.dig('observed','window') == 'Original window' && context.dig('observed','field').include?('AXTextArea')
visual = ask('spoken_window', focus: context.fetch('id'))
if visual['ok']
  require 'base64'
  abort 'Missing JPEG' unless Base64.strict_decode64(visual.fetch('jpeg')).start_with?("\xff\xd8".b)
else
  abort visual.inspect unless visual.fetch('error').include?('Screen & System Audio Recording')
end
File.write('switch','')
wait { state.call.first == 'second' }
text = "  requested 🌍\nsecond line"
reply = ask('paste_text', focus: context.fetch('id'), text: text)
abort reply.inspect unless reply['ok']
wait { state.call == ['first',text,''] }
File.write('secure','')
wait { state.call.first == 'second' }
ask('remember_focus')
secure = ask('spoken_context')
abort secure.inspect unless secure.dig('observed','field') == 'protected; omitted' && !secure.to_s.include?('private-password-marker')
abort 'Protected target accepted' if ask('paste_text', focus: secure.fetch('id'), text: 'wrong')['ok'] || ask('spoken_window', focus: secure.fetch('id'))['ok']
abort 'Stale target accepted' if ask('paste_text', focus: context.fetch('id'), text: 'wrong')['ok']
puts 'Verified exact paste after switching windows, no Return, untouched second editor, and protected/stale target refusal.'
RUBY
