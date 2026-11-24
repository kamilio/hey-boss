# Hey Clipboard

A native Liquid Glass clipboard picker, using the same teal accents and rounded typography as Hey Talk. Keeps the last 40 text and image clips on this Mac; pinned clips stay until deleted. Search matches full text and the source app. Repeated copies move to the top without duplicates.

**Press ⌘⇧V and release** to paste the newest entry. Keep holding the modifiers and tap only V to move down through older entries, including images; release both Command and Shift to paste the selection. Releasing just one modifier keeps the picker open, and V still advances. Escape cancels without pasting. Click the menu-bar icon for search, or use ↑/↓ and Enter. Double-click also pastes. Escape dismisses; Command+Delete removes the selected entry. The menu-bar icon opens the picker; right-click it to pause capture or quit. **Hyper+V** still recovers the last Hey Talk dictation.

```sh
packages/hey-clipboard/install.sh
hey-clipboard --help
```

Installation gracefully quits Flycut, imports its saved history and favorites on first install, and starts Hey Clipboard at login. Flycut and its saved data remain installed. Builds use the repo's existing persistent signing certificate. Install Hey Talk too: pasting reuses its Accessibility permission. Copying and history work without Hey Talk.

The shortcut opens a small, title-free picker with no search or management controls. The menu-bar icon and `hey-clipboard show` open the larger history view, with search focused immediately, pins, and full-text previews.

Both views use native `NSGlassEffectView` on macOS 26+, with a system material fallback on macOS 15. No webview.

It stays out of the Dock and Command-Tab. Arrow keys, Enter, and Escape also work while Command+Shift are held.

## CLI

Commands return JSON with `ok`; errors exit 1. `list` and `search` return stable IDs, previews, pasteboard types, source apps, dates, and pinned state. `get` returns the full text or a local `image_path`; `get ID --raw` emits the text or original image bytes.

```sh
hey-clipboard status
hey-clipboard search 'deployment URL'
hey-clipboard get ID --raw
hey-clipboard copy ID
printf 'some text' | hey-clipboard copy --stdin
hey-clipboard paste ID --app com.t3tools.t3code
hey-clipboard pin ID
hey-clipboard delete ID
hey-clipboard pause
hey-clipboard resume
```

`paste` requires an explicit running destination app, activates it, and sends Command-V without Enter. A focus or clipboard change cancels the keypress. `clear` deletes all history including pins. `import-flycut` can repeat safely without duplicating entries. `show` opens the picker.

History and the same-user control socket live in `~/Library/Application Support/hey-clipboard/`, with directory mode 700 and file/socket mode 600. Text entries are at most 1 MiB each. PNG and TIFF images are kept as separate files, at most 32 MiB each, with thumbnails in both pickers; deleting or aging out an entry removes its image file. File references are not stored. No cloud sync. Capture skips transient, concealed, auto-generated, and password-manager pasteboard markers. Restoring the clipboard after Hey Talk dictation does not create another history entry. Explicit Copy from dictation History is a normal copy and is recorded.

```sh
swift test --package-path packages/hey-clipboard
```
