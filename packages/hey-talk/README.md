# hey-talk

Native macOS dictation, controlled from the CLI. Press Hyper+R to start;
press it again to stop and paste. Hyper+E toggles Auto Submit for the focused website or desktop app:
on sends Enter after pasting; off only pastes. `latticehq.com` is paste-only by default.
Caps Lock must already be mapped
to Hyper (Control + Command + Option + Shift). Click the pill to toggle recording,
drag to move it, or press Esc to discard. Clicking the pill follows the same
Auto Submit setting. CLI commands without `--submit` paste by default.

Brief focus changes are silent. If the original destination is still unfocused after 0.8 seconds, the pill shows **Waiting to paste**.
Return to the original app to paste automatically. In T3, return to the starting
thread **and focus the original field**. Esc or × copies the waiting transcript to the clipboard and cancels the automatic paste;
starting another dictation replaces it. The transcript stays in History.

**Return to Field Before Pasting** is on by default. When text is ready, Hey Talk
brings the destination window forward and focuses the original input. It can
find a replaced composer in T3 or Slack, or reselect a cmux terminal tab in
the same workspace. After two seconds it stops trying to move focus. While a
paste is pending, it only checks whether you’ve returned; Esc cancels the wait.
Turn it off in the waveform menu or the pill’s right-click menu to return manually.

**Hyper+V toggles Auto mode** (Caps Lock + V with the same Hyper mapping).
The compact pill has a soft purple glow and purple recording bars. Focus T3's
**Message** field to listen. Speaking starts a recording; pausing for about
2½ seconds transcribes, pastes, and submits. Auto uses the same local speech
detector as voice conversations, including buffered opening audio. Silent
listening is not saved. Plain Enter sends a recording immediately and passes
through normally while listening. It starts listening again after submitting if an allowed
input still has focus. If no speech is detected, a brief notice appears and
listening resumes. The next plain Enter in that input passes through to the app
without stopping recording. Shift+Enter still inserts a newline. In T3, moving to another
thread leaves the destination pinned. Leaving the original field for two seconds
finishes the T3 recording. In other allowed apps, finishing chooses the
destination and leaving allowed inputs for two seconds discards the recording.
Escape or turning Auto off discards immediately. After Escape, leave and refocus
the field to resume. Hyper+R and clicking the pill still start and stop dictation;
Hyper+E changes Auto Submit without interrupting recording.
A small three-second drawer says “Auto mode on” or “Auto mode off” when toggled.

Auto starts off and remembers your choice. T3 is the initial allowed app;
its search and settings fields do not start listening. Other allowed apps use
editable text fields, excluding password fields. Accessibility permission is
required. Auto waits for the destination field before pasting or submitting.
The existing app and website Enter exceptions also apply in Auto.

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
keeps Hey Talk and its shortcuts running. Hover over the status-bar icon to see
whether the pill is visible or hidden.

**AI Correction** in the waveform menu or the pill's right-click menu toggles
punctuation and typo correction and filler removal with OpenAI GPT-6 Luna. It starts on
and remembers your choice. A tiny red brain beside the waveform means AI is off;
there is no indicator when it is on. `hey-talk correction on|off|toggle` controls the same setting.
A matching red Return symbol means Auto Submit is off. While idle it follows the
focused app or website’s Enter exceptions, so an excluded app such as Slack shows red even
before recording. During dictation it also reflects paste-only versus submit;
Hyper+R and Auto Dictation cannot override an exception.
When enabled, the transcript, existing vocabulary, visible conversation text,
and the current input
from the dictation's target window go to the existing
Hey Proxy at `127.0.0.1:8080` as `gpt-6-luna`, requesting priority service with reasoning disabled and storage off.
The proxy's configured routing, model rewrites, and authentication remain authoritative.
Audio stays local. Correction applies before History, copy, paste, or submission,
including file transcription. A response can stream for up to 30 seconds, with at most eight seconds
between updates. If correction fails, a notice explains why and the original transcript is kept. Escape discards pending correction too.
Paste is the default. In a T3 thread composer, say “Rename this thread to Release Notes”
to change the title. Hey Talk identifies the original chat through Accessibility and
uses T3’s local server API without submitting a message or touching the clipboard.
Ask for a new thread in your own words. “Start a new thread and check whether publishing works”
opens a thread in the current project and puts the request in its composer. Auto Submit
controls whether it sends. Name a project to switch projects, or leave out the task for a blank thread.
“Go to chat three” switches to sidebar position 3 (the same slot as ⌘3). “Go to Publish Artifact conversation”
reads the current sidebar first and resolves the title among positions 1–9. If the list changes before navigation, the action stops.
“Settle this thread”, “Pin this thread”, and “Unpin this thread” check the current state, so repeating a command is safe.
A small confirmation appears at the top of the display after a T3 action, then disappears on its own. Paste, clear, and sidebar lookups stay quiet.
Rename, pin/unpin, and settle use T3’s protocol-2 API with five-minute credentials from `~/.t3/bin/t3`, held only in memory.
Named navigation uses T3’s deep links; numbered navigation uses its ⌘1–9 shortcuts. Server actions currently support local T3 threads.
These tools are grouped under `t3_` and available only in a T3 thread composer. The model receives the destination app's
name and bundle identifier, plus the current T3 thread title, refreshed after navigation. Auto Dictation resumes listening
after switching, pinning, unpinning, or settling.
In any recognized editable field, say “clear this input” to select and delete its contents without submitting.
Hey Talk gives preparatory tool results back to the model so it can take the next requested action. For example,
“clear this input, then paste Check publishing” clears the field before inserting the new text. With Auto Submit on,
the paste tool presses Enter. With it off, the text stays in the field. Paste ends the sequence without another model request.
A blocked action stops the sequence; completed actions stay in History.
With `--no-paste`, only the paste result is available.

Context is a spelling hint; the transcript takes priority. Hey Talk takes one
background snapshot after the microphone is ready and uses it for correction even
if you change tabs before stopping. Recording never waits for context capture.
Capture uses macOS Accessibility with a
200 ms budget and an 8,000-character cap. It skips password fields, unrelated editors, controls, navigation trees, and
clipped or off-screen text; cmux's rendered terminal viewport is included.
The focused input is read before the rest of the window and has priority within the character cap. Named regions and message headings separate the captured text into sections.
T3 and Slack capture keep conversations and the current input, excluding navigation
and workspace status text. The focused input or message composer appears under **Current input**.
Missing context does not block correction. File transcription never reads app context.
Press **Hyper+M** (Control+Command+Option+Shift+M), or choose **Chat with Context** from the waveform menu, to start a new Hey Boss chat and listen on macOS 26. The source app appears above the message field; click it to inspect or remove the capture. Your next spoken or typed message shows a preview; click its paperclip to read the full capture. Ordinary chats send messages and captured text through Hey Proxy using the `gpt-6.1-sol` alias and its routing rules; folder chats send them to your configured Codex provider. If the app exposes no readable text, voice chat still opens. With the chat focused, **Hyper+R** or a click on its voice bubble starts voice, sends the current spoken message while recording, or turns voice off while waiting for speech. Elsewhere, **Hyper+R** still toggles Hey Talk dictation.
Inspect exactly what was sent with `hey-talk last --json | jq -r '.app_context'`;
`history --json` also includes it. `null` means no context was attached.
Context is saved locally with the original and final transcript, with the same
user-only file permissions; it is not included in routine status output.
Result JSON includes correction status (`off`, `applied`, `fallback`, or `partial` if correction failed after a tool ran), timing,
and `original_text` alongside the final `text`, including when correction is off or fails.
Explicit “new line” or `\n` commands produce line breaks in corrected text.

## Read aloud

In T3, hover over a paragraph for a faint glass outline and a small **Listen**
capsule fixed beside the paragraph. The whole capsule is clickable and brightens
under the pointer. A generous hover area bridges the paragraph and capsule, with
a 1.5-second grace period when leaving it. Pointer motion does not move the
button or require holding still. **Hyper+S**
plays or pauses it; selected text takes priority. **Hyper+T** turns the feature
on or off. Both shortcuts belong to allowlisted apps only, initially T3.
Escape stops playback. Editors, buttons, toolbars and password fields are excluded.
Right-click the main waveform bubble for **Dictation**, **Speech**, and **Setup**
sections. Checkmarks show the current modes, with shortcuts aligned on the right.
Click anywhere outside the menu to close it.

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

Use `hey-talk transcribe-file recording.m4a` for a local transcript of any length.
Add `--json` for `{"ok":true,"text":"…"}`. It runs independently of dictation
and does not use AI correction, copy to the clipboard, or write history.

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

Suppress Enter in desktop apps or on websites, even with Hyper+R:

```sh
hey-talk enter-exceptions add com.tinyspeck.slackmacgap
hey-talk enter-exceptions                         # List exceptions
hey-talk enter-exceptions remove com.tinyspeck.slackmacgap
hey-talk enter-exceptions sites                   # List excluded websites
hey-talk enter-exceptions sites add example.org
hey-talk enter-exceptions sites remove example.org
```

Changes apply immediately and persist across restarts in the app's macOS
preferences. Website rules match a hostname and its subdomains across browsers.
The default is `latticehq.com`. Hyper+E changes the current
host; use the CLI to remove a parent-domain or browser-wide rule. On a browser
page whose host cannot be identified, Hey Talk only pastes.

In Chrome and Safari, AI Correction and Chat with Context read the active
website’s hostname and visible page text through Accessibility. They omit the
address bar, other tabs, and the page URL’s path, query, and fragment.

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

Right-click the pill and choose **Dictation History…**, the first item. During recording, the window opens without taking focus from your input. You can also use the waveform menu-bar icon or run `hey-talk
history show`. Long transcripts open as previews; choose **Show full transcript** to expand one. Copy always copies the entire transcript. Expand **Original
transcript** to inspect and copy the text before AI correction. Search matches
both versions. `hey-talk last --original` prints the latest original;
`last --json` and `history --json` include both `text` and `original_text`.
Older entries have `original_text: null` because their originals were not saved.
Closing this window with ⌘W leaves dictation running. Standard copy, paste, cut, select-all,
and undo commands work in native text fields. Select part of any transcript
and press ⌘C, or use its Copy button for the whole text. The search field filters
loaded entries. macOS 26 windows use native Liquid Glass. Notices expand inside
the floating capsule; `status --json` keeps the full error details.
Older entries load in pages. Saved dictations older than 14 days are removed when History is read or a dictation is saved. `history` prints the latest 50 entries; `history --before ID`
reads the next page using the `next_before` cursor from JSON output.

Completed microphone and file transcriptions are saved before any paste attempt
in `~/Library/Application Support/hey-talk/history.sqlite3`, readable only by
your user. Canceling during capture discards the recording. If saving fails, the window keeps the
text in memory and marks it **Unsaved** so you can copy it before quitting.

Allow microphone access when macOS asks. For automatic paste, choose
**Paste Permissions…** in the waveform menu or the floating capsule’s right-click
menu, or run `hey-talk permissions accessibility`. A paste-permission error also
has an **Enable Accessibility** button and stays visible until dismissed
or recording resumes. The guide opens the Accessibility settings page, reveals
the installed app in Finder if it needs adding, and detects when permission is on.
In the macOS list, enable **Hey Talk**. If it is missing, click **+**, press
**⇧⌘G**, enter `~/Applications`, and choose **Hey Talk.app**. Without
that permission, dictation copies to the clipboard. While waiting for focus, the clipboard stays untouched.
The stop response reports `delivery: "waiting"`; status reports `waiting_to_paste`. `--no-paste` and file
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

Microphone recordings are limited to five minutes. When you finish one, Hey Talk saves
the untrimmed 16 kHz mono audio as a lossless WAV in
`~/Library/Application Support/hey-talk/recordings/`, even if transcription fails.
Only the latest five are kept; the highest numbered file is the newest. Canceling
during capture and transcribing existing files leave this folder unchanged.
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
HEY_TALK_EVAL=1 swift test --filter CorrectionEvalTests # Live Luna, requires the saved mini/context evals under output/
HEY_TALK_T3_NEW_TEST=1 swift test --filter T3NewThreadTests # Focus a T3 composer; set HEY_TALK_T3_PROJECT to test another project
HEY_TALK_T3_TEST=1 swift test --filter T3RenameTests # Focus a T3 composer; confirms its existing title
HEY_TALK_T3_TOOLS_TEST=1 swift test --filter T3ToolsTests.testLiveNavigationPreservesDraftAndClipboard # Auto off; restores navigation
HEY_TALK_T3_SERVER_THREAD=THREAD_UUID swift test --filter T3ToolsTests.testLiveServerPinUnpinAndSettleState # Restores pin state; checks an already settled thread
./test-context.sh # Installed app: change visible text during microphone dictation
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
check it listens without recording; Escape or focus a search field and check it stops.
Refocus, dictate, and pause: exactly one message should submit and the
microphone should reopen. Move between inputs while speaking and verify the
recording stays pinned to its starting T3 field; return there to receive it. Also check Hyper+E toggles
Auto Submit without interrupting recording, Hyper+R respects that setting, and switching away after paste but before Enter never submits in the new field.

See [THIRD_PARTY.md](THIRD_PARTY.md) for the retained OpenWhispr, sherpa-onnx,
ONNX Runtime, and Orukeet licenses.
