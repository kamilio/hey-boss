# hey-talk

Native macOS dictation, controlled from the CLI. Press Hyper+E or Hyper+R to start;
press Hyper+E to stop and paste, or Hyper+R to stop, paste, and press Enter.
The shortcut used to stop chooses the behavior. Caps Lock must already be mapped
to Hyper (Control + Command + Option + Shift). Click the pill to toggle recording,
drag to move it, or press Esc to discard. Clicking to stop keeps the starting
shortcut's behavior; clicking to start and CLI commands without `--submit` paste by default.

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
It expands to show live teal audio levels while recording, then a thin indigo
progress rim while transcribing. Progress is estimated from recording length,
includes AI correction when enabled, and gently advances if processing runs long.
The rim completes when the transcript is ready. Accent colors match the shared Hey Boss design tokens.
“Connecting…” appears only if opening the microphone takes more than 700 ms.
With Accessibility enabled, the bubble lifts above the bottom Dock when it opens
and returns to its previous position when the Dock hides.
macOS 26 uses native Liquid Glass with a fine reflective edge and soft shadow;
macOS 15–25 uses a native translucent surface. There is no web renderer or account.
Recording is silent on every microphone. The capsule provides the start and stop
indicators.
The waveform status-bar menu offers **Hide Pill** / **Show Pill**; hiding the pill
keeps Hey Talk and its shortcuts running. A tiny red speaker-off marker on the
pill and status-bar icon means **Read Aloud** is off. Toggle it from either menu;
dictation still works. Hover over the status-bar icon to see both states.

**AI Correction** in the waveform menu or the pill's right-click menu toggles
punctuation and typo correction and filler removal with OpenAI GPT-6 Luna. A tiny teal dot beside
the waveform mark means it is on; no dot means it is off. It starts off and
remembers your choice. `hey-talk correction on|off|toggle` controls the same setting.
When enabled, the transcript and existing vocabulary list go to the existing
Hey Proxy at `127.0.0.1:8080` as `gpt-6-luna`, with reasoning disabled and storage off.
The proxy's configured routing, model rewrites, and authentication remain authoritative.
Audio stays local. Correction applies before History, copy, paste, or submission,
including file transcription. If it fails or exceeds eight seconds, the original
transcript is kept with a brief notice. Escape discards pending correction too.
Result JSON includes correction status (`off`, `applied`, or `fallback`), timing,
and `original_text` alongside the final `text`, including when correction is off or fails.

## Read aloud

In T3, hover over a paragraph for a faint glass outline and a small **Listen**
capsule fixed beside the paragraph. The whole capsule is clickable and brightens
under the pointer. A generous hover area bridges the paragraph and capsule, with
a 1.5-second grace period when leaving it. Pointer motion does not move the
button or require holding still. **Hyper+S**
plays or pauses it; selected text takes priority. **Hyper+T** turns the feature
on or off. Both shortcuts belong to allowlisted apps only, initially T3.
Escape stops playback. Editors, buttons, toolbars and password fields are excluded.
Right-click the main waveform bubble to toggle **Read Aloud · Hyper+T** or
**Auto Dictation · Hyper+V**. Checkmarks show the current modes.

The Listen capsule shows how many paragraphs will be read. Playback continues
through subsequent readable paragraphs, including paragraphs after a list or
nested text block in the same section,
stopping at controls or unrelated containers (up to 64 paragraphs / 20,000 UTF-16
units). Selected text is read on its own. Capture uses shared macOS Accessibility
capabilities, without T3-specific selectors.

Playback opens a movable, nonactivating glass drawer beside the starting paragraph
with a frozen copy of the passage. Scrolling does not change the snapshot.
A click-through glass outline follows the current source paragraph; it clips to
the visible area and disappears when the source is off-screen or changes.
The drawer shows the current paragraph number within the group. The spoken word
gets a subtle glass lens with 3.5% enlargement inside the drawer,
without reflowing the text. Reduce Motion disables enlargement; Reduce Transparency
uses a solid highlight. The drawer retains the font,
bold, italic and color attributes supplied by Accessibility; it cannot reproduce
CSS details that T3 does not expose. Use the disclosure button to collapse it,
or the speed button to cycle from 0.75× to 2×. Playback starts at 1.25×.

Supertonic 3 generates short English audio chunks locally while earlier chunks
play. Only two chunks are queued. The model warms when entering an allowed app
and is released after ten idle minutes. Recognition supplies word timings after
audio starts; its model loads in the background so it cannot delay first audio
or the next chunk. The first chunks may use a soft phrase lens until word timing
is ready. Unmatched numbers or code terms also use that softer phrase highlight.
Leaving the allowlist, turning the feature off, or starting manual dictation
stops playback. Auto dictation pauses during reading.

```sh
hey-talk speech                          # State and allowed apps
hey-talk speech on                       # Or off
hey-talk speech add com.example.Reader
hey-talk speech remove com.example.Reader
hey-talk speech target --json             # Explicitly inspect selected/hovered text
hey-talk speech play                      # Same play/pause behavior as Hyper+S
hey-talk speech pause
hey-talk speech stop
```

Routine status and logs do not contain captured text. Speech and timings stay
on this Mac. The engine interface supports a future remote provider; no remote
provider or network inference is configured in this build.

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
Installation and login startup are per Mac. Installing Hey Boss on another
machine does not enable Hey Talk there.
Hey Talk is intentionally uninstalled on the second Mac
(`kamils-macbook-pro.local`, also named `Mac.lan`). That Mac is excluded from
Hey Talk installs and updates, including the project's usual all-machines rollout.

## Daily use

```sh
hey-talk devices
hey-talk input default
hey-talk start --no-overlay --no-paste
hey-talk stop --json
hey-talk cancel
hey-talk toggle --submit                    # Same start/stop + submit behavior as Hyper+R
hey-talk overlay show
hey-talk history show
hey-talk history --json
hey-talk warm
hey-talk auto                               # Show mode and allowed apps
hey-talk auto on                            # Also available in the menu or Hyper+V
hey-talk auto off
hey-talk auto toggle                        # App-side toggle; no status/read race
hey-talk auto add com.example.Editor        # Allow another desktop app
hey-talk auto remove com.example.Editor
```

Karabiner `shell_command` and LinearMouse `run` can call these commands directly,
without simulating a keyboard shortcut. `--submit` also works with `start`, `stop`,
and `record`; the stopping command chooses submission when it specifies the flag.
Ordinary `toggle` preserves the current recording's delivery mode. Per-app Enter
exceptions and focus-change protection still apply. `--submit` rejects `--no-paste`.

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

Vocabulary is shared through Hey Boss's existing machine sync. Install a current
Hey Boss on participating machines. Changes made offline merge when they reconnect;
removals survive a later import of an old list. The first use imports your saved
words, or [vocabulary.md](vocabulary.md) on a fresh installation. Empty lists survive upgrades.

Quote phrases. Adding a term updates its preferred spelling; removal ignores case.
The CLI refreshes the local copy immediately and works while Hey Talk is stopped.
The running app checks for shared changes every 15 seconds, off the recording thread.
Inference reads `~/Library/Application Support/hey-talk/vocabulary.md`, so dictation
keeps working offline. Use the CLI to edit the shared list; this Markdown file is
its local cache. `--json` reports `source: fleet` or a cached result with `sync_error`.
If Hey Boss is unavailable, edits fail visibly and the last good copy stays usable.
Only the word list is shared; recordings, history, Auto mode and login startup stay
on each Mac. Hints favor terms but do not guarantee spelling or replace text afterward.

Open **Dictation History…** from the waveform menu-bar icon, or run `hey-talk
history show`. Each transcript has a timestamp and Copy button. Expand **Original
transcript** to inspect and copy the text before AI correction. Search matches
both versions. `hey-talk last --original` prints the latest original;
`last --json` and `history --json` include both `text` and `original_text`.
Older entries have `original_text: null` because their originals were not saved.
Closing this window with ⌘W leaves dictation running. Standard copy, paste, cut, select-all,
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
graph to interfere with Bluetooth profile changes. The default input follows macOS
even during recording. Input changes, interruptions, and stalled or zero-filled
streams reopen capture automatically while retaining audio already received.
Recording becomes ready after the first nonzero audio buffer; a recovery that
cannot restore sound fails visibly. `status --json` includes `active_input` and
`capture_restarts`. An explicitly selected input stays selected.
If the default Bluetooth microphone stays silent after one retry, capture can
use the unmuted built-in microphone for that recording, with a notice and
`fallback_from` in status. This does not change the system input or saved choice;
fallback is disabled when the laptop lid is closed or the input reports mute.
With the MacBook lid closed, choose an external microphone as the system input.
A separate disposable process runs Orukeet v0.1.0 INT8 through sherpa-onnx and
ONNX Runtime. Private pipes carry PCM; the CLI uses a same-user Unix socket
with mode 0600. Hey Talk opens no network listener. Recognition runs locally;
optional AI correction sends text through the local proxy as described above.
An inference crash retries once without restarting the recording service.

Audio is bounded to five minutes, held only in memory, and discarded on cancel.
The microphone connection stays open for five minutes after a recording stops
or is cancelled, so repeated dictations can reuse the Bluetooth connection.
Idle audio is discarded before metering, buffering, or transcription. The macOS
microphone indicator stays lit while the connection is warm. Sleep, input changes,
disconnects, and expiry close it; idle capture never opens a replacement device.
`status --json` reports this as `microphone_keepalive`.
Inference stays warm for ten minutes, then releases model memory. Waveform updates
run only during visible recording. Bluetooth can still take roughly two seconds
to open its microphone profile on first use. Readiness requires fresh audio from
the new recording, not cached idle samples;
The bar appears immediately with “Connecting…” while waiting for sound, then
shows the live waveform. Click the bar to stop; Esc cancels.
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
./test-speech.sh # Real synthesis, recognition, word timing and playback state
# To test the installed app instead:
HEY_TALK_TEST_CLI="$HOME/.local/bin/hey-talk" ./test-integration.sh
# Optional hardware check; captures and discards the selected microphone:
./test-microphone.sh
./test-input-switching.sh # Requires two microphones; restores the system input
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
