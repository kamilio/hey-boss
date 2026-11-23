# Sticky

A native floating Mac note with Liquid Glass on macOS 26. **Hyper+N**
(Control+Command+Option+Shift+N) shows or hides
all notes without switching away from your current app. Sticky starts at login.

One **Daily Focus** note is created by default. The first Markdown H1 becomes
the window title; the rest renders as the document. Drag the header to move,
resize from the bottom-right corner, and click **×** to hide. That is the only
button. The note floats above other windows like the dictation pill, never takes
keyboard focus, and stays out of Command-Tab and the Dock. Clicking a link opens
its destination app. Closing or hiding never deletes text. Positions and sizes persist
separately for each display. Disconnecting a display uses the laptop position;
reconnecting restores the preferred display. New positions start near the top-right.

Everything else is on the CLI. `sticky edit` explicitly opens a separate, large
Markdown editor. **⌘S** saves and updates the floating note; closing the editor
asks to save or discard unsaved changes. Opening that editor is the only action
that activates Sticky.

```sh
packages/sticky/install.sh
sticky set daily-focus.md
sticky set - < daily-focus.md
sticky show
sticky edit
sticky preview daily-focus.md --output preview.png
sticky export --output focus.png --appearance dark
sticky new another-note.md
sticky list
sticky --help
```

`set` replaces the default `focus` note; `--id` selects another note. `new`
returns the new ID. Updates refuse to replace a note with unsaved editor changes.
Notes are private UTF-8 Markdown files under
`~/Library/Application Support/Sticky/Notes/`. Use `sticky set` for live updates;
`sticky get` returns the saved source. The CLI uses a user-only local socket.
No account, network service, or AI prompt is involved.

Markdown includes lists, task lists, tables, quotes, code, images, footnotes,
and clickable links. Mermaid fences render charts and diagrams from a bundled
renderer, offline. Raw HTML displays as text. Remote images load from their URLs;
links open in the default app. Relative links and local images are not supported;
use absolute links and HTTPS image URLs.

`preview` renders a file without changing saved notes; `export` renders a saved
note. Both produce a 2× PNG using the same WebKit renderer and typography as the
window, wait for images/charts, and report failures with a nonzero exit code.
PNG exports have a stable warm glass backing, transparent rounded corners, and
the complete document, without capturing desktop content. The live window uses
native Liquid Glass. `--width` accepts 300–1200 points; maximum export height is
8192 content points. macOS with a logged-in desktop session is required.

Build requires Swift 6.2+, Rust, and Node/npm. Rust handles Markdown parsing;
AppKit owns windows, editing, persistence, global shortcuts, and PNG export.
Build output is local to this package. Installation only updates Sticky.

```sh
cargo test --manifest-path packages/sticky/renderer/Cargo.toml
# After build.sh has built the Rust library and diagram bundle:
(cd packages/sticky && swift test)
packages/sticky/test-integration.sh
```
