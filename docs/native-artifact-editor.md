# Focused artifact and Markdown editor

Desktop **Edit** opens a native macOS writing window. **Edit in browser** remains in the document’s More actions menu. The web reader refreshes after native saves without replacing a browser draft.

`hey-boss artifact open` opens the library; `open --new` starts a note; `edit ID` opens an artifact; `open --file notes.md` edits a local UTF-8 Markdown file. Artifact commands with text still require `--if-version`.

The editor starts in focus mode with a native Liquid Glass window toolbar, an editable document name, and a writing surface that follows the window width. Window size is remembered. The app appears in the Dock and ⌘Tab; ⌘` cycles document windows. The library uses a system sidebar; outline, formatting, and reading controls stay in the toolbar. ⌘P opens the library, ⌘⇧F toggles focus, and ⌘E switches writing/reading. ⌘N creates a note, ⌘O opens Markdown, ⌘⇧S exports a copy, and ⌘W closes. Standard selection, clipboard, undo/redo and Find shortcuts use AppKit. The Format menu lists bold (⌘B), italic (⌘I), links (⌘K), code (⌘⇧C), headings (⌘1–6), lists and tasks.

Changes save automatically after a brief typing pause and when the window loses focus or closes. The editor journals pending text and a stable request ID before sending a revision-checked write through the Rust artifact API. Closed notes continue syncing; unsent drafts reopen after an app restart. A conflicting external edit is preserved: export a copy or reload the saved version. Reload keeps the old draft in the private recovery directory.

Artifact Markdown is limited to 1 MiB; local files support 64 MiB. Editing uses one native text storage with noncontiguous layout and visible-range syntax coloring. Preview parsing runs off the UI thread through the shared Rust Markdown renderer.
