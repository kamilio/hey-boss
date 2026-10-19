# hey-talk

Native macOS dictation, controlled from the CLI. Hold Caps Lock + E to record;
release to transcribe and paste. Caps Lock must already be mapped to Hyper
(Control + Command + Option + Shift). Click the pill to toggle recording, drag
to move it, or press Esc to discard.

The recording pill follows OpenWhispr's dimensions, waveform, transitions,
processing glow, and sound cues. macOS 26 uses real Liquid Glass; macOS 15–25
uses a native translucent surface. There is no web renderer or account.

## Build and run

Requires macOS 15+ and the Xcode 26 toolchain (Swift 6.2+). Build on the Mac that
will run it. Runtime downloads are pinned and SHA-256 verified.

```sh
cd ~/Workspace/hey-boss/packages/hey-talk
./build.sh
"dist/Hey Talk.app/Contents/MacOS/hey-talk" --help
"dist/Hey Talk.app/Contents/MacOS/hey-talk" record --no-paste
"dist/Hey Talk.app/Contents/MacOS/hey-talk" transcribe recording.wav --json
```

The development build can reuse OpenWhispr's existing Orukeet download.
`./prepare-model.sh` makes an independent, verified copy or downloads the model.

Installation is explicit and local to the current Mac:

```sh
./install.sh
~/.local/bin/hey-talk doctor
```

This installs `~/Applications/Hey Talk.app` and `~/.local/bin/hey-talk`.
No login agent or fleet deployment is created. Commands launch the app through
Launch Services when needed; `hey-talk quit` stops it and its inference worker.
Quit a development instance before rebuilding its bundle.

## Daily use

```sh
hey-talk devices
hey-talk input default
hey-talk start --no-overlay --no-paste
hey-talk stop --json
hey-talk cancel
hey-talk overlay show
hey-talk warm
```

`record` stops on Enter or Ctrl-C. `stop` returns the transcript; `--json` also
reports capture latency, inference duration, and real-time factor. `status
--json` exposes readiness, measured microphone levels, hotkey registration,
and process IDs. `last` reads the most recent transcript from memory.

Allow microphone access when macOS asks. For automatic paste, run `hey-talk
permissions accessibility` and enable **Hey Talk** in System Settings. Without
that permission, dictation copies to the clipboard. If focus changes while
transcribing, it also leaves the text on the clipboard. `--no-paste` and file
transcription leave the clipboard untouched. Locally rebuilt app signatures
can require permission again. Quit OpenWhispr if it owns the same hotkey.

## Runtime and privacy

The Swift desktop process owns capture, keyboard shortcuts, and AppKit UI.
A separate disposable process runs Orukeet v0.1.0 INT8 through sherpa-onnx and
ONNX Runtime. Private pipes carry PCM; the CLI uses a same-user Unix socket
with mode 0600. No network listener or inference network request is used.
An inference crash retries once without restarting the recording service.

Audio is bounded to five minutes, held only in memory, and discarded on cancel.
The microphone closes when recording stops. Inference stays warm for ten
minutes, then releases model memory. Waveform updates run only during visible
recording. Bluetooth can take roughly two seconds to open its microphone profile
on first use; subsequent starts measured around 90 ms on the tested headset.
Startup runs off the UI thread and remains cancellable.

On an M5 Pro, a 5.05-second synthetic speech clip decoded in 138–203 ms with a
warm model (25–36× real time). The app bundle is about 34 MB; the idle desktop
process used about 42 MB RSS. The warmed model worker used about 1.4 GB RSS.
These are local measurements, not latency guarantees.

## Verification

```sh
swift test
./test-integration.sh
```

Integration tests use synthetic speech, check transcription, kill the inference
worker to verify recovery, and cancel work in flight. They do not use the mic
or paste text. The hardware microphone and global shortcut still depend on
macOS permission and the user's keyboard mapping.

See [THIRD_PARTY.md](THIRD_PARTY.md) for the retained OpenWhispr, sherpa-onnx,
ONNX Runtime, and Orukeet licenses.
