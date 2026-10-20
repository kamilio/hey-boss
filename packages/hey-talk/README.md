# hey-talk

Native macOS dictation, controlled from the CLI. Press Hyper+E or Hyper+R to start;
press Hyper+E to stop and paste, or Hyper+R to stop, paste, and press Enter.
The shortcut used to stop chooses the behavior. Caps Lock must already be mapped
to Hyper (Control + Command + Option + Shift). Click the pill to toggle recording,
drag to move it, or press Esc to discard. Clicking to stop keeps the starting
shortcut's behavior; recordings started by clicking or the CLI only paste.

**Hyper+V toggles Auto mode** (Caps Lock + V with the same Hyper mapping).
The compact pill has a soft purple glow and purple recording bars. Focus T3's
**Message** field to start recording, then press plain Enter to transcribe,
paste, and submit. It starts listening again after submitting if an allowed
input still has focus. If no speech is detected, a brief notice appears and
listening resumes. The next plain Enter in that input passes through to the app
without stopping recording. Shift+Enter still inserts a newline. You can move between
allowed inputs while speaking: Enter chooses the destination. Leaving the inputs
for more than two seconds discards the recording; shorter gaps keep it running.
Escape or turning Auto off discards immediately. After Escape, leave and refocus
the field to resume. Hyper+E, Hyper+R, and clicking the
pill still work; the stopping shortcut chooses paste or submit as usual.
A small three-second drawer says “Auto mode on” or “Auto mode off” when toggled.

Auto starts off and remembers your choice. T3 is the initial allowed app;
its search and settings fields do not start recording. Other allowed apps use
editable text fields, excluding password fields. Accessibility permission is
required. A focus change during transcription keeps the result in History and
on the clipboard instead of pasting or submitting into another field or thread.
The existing per-app Enter exceptions also apply in Auto.

The floating control is a compact glass capsule with an indigo waveform mark.
It expands to show live teal audio levels while recording, then a quiet indigo
glow while transcribing. Accent colors match the shared Hey Boss design tokens.
macOS 26 uses native Liquid Glass with a fine reflective edge and soft shadow;
macOS 15–25 uses a native translucent surface. There is no web renderer or account.
Recording is silent on every microphone. The capsule provides the start and stop
indicators.

## Build and run

Requires macOS 15+ and the Xcode 26 toolchain (Swift 6.2+). Build on the Mac that
will run it. Runtime downloads are pinned and SHA-256 verified.

```sh
cd ~/Workspace/hey-boss/packages/hey-talk
OP_ACCOUNT=my.1password.com ./setup-signing.sh  # Once per build machine
./build.sh
"dist/Hey Talk.app/Contents/MacOS/hey-talk" --help
"dist/Hey Talk.app/Contents/MacOS/hey-talk" record --no-paste
"dist/Hey Talk.app/Contents/MacOS/hey-talk" transcribe recording.wav --json
```

Release builds use a persistent local signing certificate backed up in the
private 1Password item **Hey Talk local code signing**. Restore that same identity
on each build machine; builds refuse to silently fall back to ad-hoc signing.
Only its public fingerprint is committed. No private keys or passwords belong in
the checkout. An installed build does not need 1Password or the signing key to run.

The development build can reuse OpenWhispr's existing Orukeet download.
`./prepare-model.sh` makes an independent, verified copy or downloads the model.

Installation is explicit and local to the current Mac:

```sh
./install.sh
~/.local/bin/hey-talk doctor
```

This installs `~/Applications/Hey Talk.app` and `~/.local/bin/hey-talk`.
The **Hey Talk…** item in Hey Boss's status-bar menu opens its history and overlay.
The installer starts the app and registers a login agent so the overlay and
shortcuts are ready after sign-in. Commands also launch the app through
Launch Services when needed; `hey-talk quit` stops it and its inference worker
until the next login or command that opens it.
For an existing installation, `./install-login.sh` enables login startup without rebuilding.
Quit a development instance before rebuilding its bundle.

## Daily use

```sh
hey-talk devices
hey-talk input default
hey-talk start --no-overlay --no-paste
hey-talk stop --json
hey-talk cancel
hey-talk overlay show
hey-talk history show
hey-talk history --json
hey-talk warm
hey-talk auto                               # Show mode and allowed apps
hey-talk auto on                            # Also available in the menu or Hyper+V
hey-talk auto off
hey-talk auto add com.example.Editor        # Allow another desktop app
hey-talk auto remove com.example.Editor
```

Suppress Enter in specific desktop apps, even with Hyper+R:

```sh
hey-talk enter-exceptions add com.tinyspeck.slackmacgap
hey-talk enter-exceptions                         # List exceptions
hey-talk enter-exceptions remove com.tinyspeck.slackmacgap
```

Changes apply immediately and persist across restarts in the app's macOS
preferences. Match the original target app's bundle ID; Slack in a browser is
not covered by the Slack desktop exception. Hyper+E remains paste-only everywhere.

`record` stops on Enter or Ctrl-C. `stop` returns the transcript; `--json` also
reports capture latency, inference duration, and real-time factor. `status
--json` exposes readiness, measured microphone levels, hotkey registration,
and process IDs. `last` reads the most recent transcript, including after restart.

Manage vocabulary hints from the CLI:

```sh
hey-talk vocabulary                         # List words and phrases
hey-talk vocabulary add Kamil Poe "T3 Code"
hey-talk vocabulary remove "T3 Code"
hey-talk vocabulary --json                  # Includes the Markdown file path
```

Changes apply to the next transcription without restarting or reloading the
model. Quote phrases. Adding an existing term updates its preferred spelling;
removal ignores case. The list lives in
`~/Library/Application Support/hey-talk/vocabulary.md`, one term per line.
The initial list comes from [vocabulary.md](vocabulary.md); app upgrades preserve
your changes, including an empty list. Hints favor these terms during local
recognition, but do not guarantee spelling or replace text afterward.

Open **Dictation History…** from the waveform menu-bar icon, or run `hey-talk
history show`. Each transcript has a timestamp and Copy button. Closing this
window with ⌘W leaves dictation running. Standard copy, paste, cut, select-all,
and undo commands work in native text fields. Select part of any transcript
and press ⌘C, or use its Copy button for the whole text. The search field filters
loaded entries. macOS 26 windows use native Liquid Glass. Notices expand inside
the floating capsule; `status --json` keeps the full error details.
Older entries load in pages; nothing expires
automatically. `history` prints the latest 50 entries; `history --before ID`
reads the next page using the `next_before` cursor from JSON output.

Completed microphone and file transcriptions are saved before any paste attempt
in `~/Library/Application Support/hey-talk/history.sqlite3`, readable only by
your user. Canceled work is never saved. If saving fails, the window keeps the
text in memory and marks it **Unsaved** so you can copy it before quitting.

Allow microphone access when macOS asks. For automatic paste, choose
**Automatic Paste…** in the waveform menu or the floating capsule’s right-click
menu, or run `hey-talk permissions accessibility`. A paste-permission error also
has an **Enable Accessibility** button and stays visible until dismissed
or recording resumes. The guide opens the Accessibility settings page, reveals
the installed app in Finder if it needs adding, and detects when permission is on.
In the macOS list, enable **Hey Talk**. If it is missing, click **+**, press
**⇧⌘G**, enter `~/Applications`, and choose **Hey Talk.app**. Without
that permission, dictation copies to the clipboard. If focus changes while
transcribing, it leaves the text on the clipboard and names the original app
in the notice. `--no-paste` and file
transcription leave the clipboard untouched. Switching from the old ad-hoc
signature to the persistent certificate requires one final permission approval.
Subsequent release builds keep the same certificate and app identity. If Settings
shows Hey Talk enabled but the guide still reports missing access, turn its switch
off and on. If that fails, remove
the old entry and add `~/Applications/Hey Talk.app` again. The guide checks the
running process, so “Automatic paste is ready” is the confirmation. Rebuilds
must use the same certificate; replacing it changes the permission identity.
Clipboard-only results include `paste_block` in JSON; `status` exposes the most
recent failure as `last_paste_block`. Notifications distinguish missing access
from an app switch or a closed target. Quit OpenWhispr if it owns the same hotkey.

## Runtime and privacy

The Swift desktop process owns capture, keyboard shortcuts, and AppKit UI.
Microphone capture uses a capture-only `AVCaptureSession`, with no output audio
graph to interfere with Bluetooth profile changes. Recording becomes ready only
after audio arrives, and a watchdog detects a microphone that stops sending data.
A separate disposable process runs Orukeet v0.1.0 INT8 through sherpa-onnx and
ONNX Runtime. Private pipes carry PCM; the CLI uses a same-user Unix socket
with mode 0600. No network listener or inference network request is used.
An inference crash retries once without restarting the recording service.

Audio is bounded to five minutes, held only in memory, and discarded on cancel.
The microphone closes when recording stops. Inference stays warm for ten
minutes, then releases model memory. Waveform updates run only during visible
recording. Bluetooth can take roughly two seconds to open its microphone profile
on first use. Ten starts on the tested headset measured 50–325 ms, with a 57 ms
median (previously 205 ms). Readiness now follows the first valid audio buffer;
the waveform appears immediately, without the previous 660 ms reveal delay.
Startup runs off the UI thread and remains cancellable.

On an M5 Pro, a 5.05-second synthetic speech clip decoded in 138–203 ms with a
warm model (25–36× real time). The app bundle is about 34 MB; the idle desktop
process used about 42 MB RSS. The warmed model worker used about 1.4 GB RSS.
These are local measurements, not latency guarantees.

## Verification

```sh
swift test
./test-signing.sh
./test-enter-exceptions.sh
./test-login.sh # Installed app: login startup and explicit quit, while idle
swift test --sanitize=thread
./test-integration.sh
./test-vocabulary.sh # Separate worker: vocabulary reload, speech, silence, speed
# To test the installed app instead:
HEY_TALK_TEST_CLI="$HOME/.local/bin/hey-talk" ./test-integration.sh
# Optional hardware check; captures and discards the selected microphone:
./test-microphone.sh
```

Run integration tests while idle: they restart Hey Talk. They run 20 synthetic
transcriptions, concurrent CLI calls, worker crashes before and during decode,
cancellation, missing files, and a main-process crash to verify durable history
and orphan cleanup. Synthetic history entries are removed afterward. Unit tests
also cover database write failure, paging, audio formats, and unresponsive clients.
Integration tests do not use the mic
or paste text. The hardware microphone and global shortcut still depend on
macOS permission and the user's keyboard mapping.

The vocabulary test leaves the running app, microphone, clipboard, preferences,
and history untouched. Set `HEY_TALK_TEST_APP` to an installed app path to test it.

Auto checks: toggle Hyper+V and inspect the glow; focus T3's Message field and
check recording starts; Escape or focus a search field and check it stops.
Refocus, dictate, and press Enter: exactly one message should submit and the
microphone should reopen. Move between inputs while speaking and verify the
recording continues; leaving them for two seconds should cancel. Also check Hyper+E pastes without submitting,
Hyper+R submits, and switching fields during transcription prevents delivery.

See [THIRD_PARTY.md](THIRD_PARTY.md) for the retained OpenWhispr, sherpa-onnx,
ONNX Runtime, and Orukeet licenses.
