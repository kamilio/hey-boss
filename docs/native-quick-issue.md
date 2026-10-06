# Native Quick Add

Press **Command+Control+Option+Shift+I** anywhere on macOS, or choose
**Quick add issue…** from the Hey Boss menu bar. An AppKit panel opens on the
active display, including over full-screen applications. No web page or embedded
browser is involved, and the registered shortcut needs no Accessibility access.

Type an issue title and **@project** to choose its destination. Suggestions match
project names and full IDs, including hidden projects. Use **↑/↓** to browse and
**Enter** or **Tab** to choose, or click a suggestion. Mentions can appear anywhere
in the title. Names containing spaces are quoted automatically; ambiguous names
use the full project ID. Email addresses remain ordinary title text; `\@` inserts
a literal `@`. Unknown projects and conflicting destinations show an inline error.

The first issue needs an explicit project. Later issues default to the last
successfully used project on that Mac. The footer previews the destination.
Press **Command+Enter** (including keypad Enter), **Enter**, or click **Create** to create an ordinary open, unassigned issue
as Boss, with no labels. A short native confirmation
shows the issue number and destination with a green checkmark, then closes after
1.8 seconds. Command+Enter submits the typed destination even while suggestions
are open; plain Enter still selects a suggestion first. Held keys and input-method
composition do not submit. Issues go to the top by default; **Add to bottom** or
**⌘⇧B** appends instead. The CLI exposes the same atomic placement with
`hey-boss issue create --title TITLE --at-top`. CLI creation also goes to the front
by default; use `--at-bottom` to append deliberately.

**Escape** dismisses suggestions first, then the panel. The close button and
switching to another application also dismiss it. Unsent text and placement stay
available until the desktop app exits. Submission temporarily disables editing
and dismissal, with a native spinner and a disabled **Creating…** button. The
panel uses the same clipped Liquid Glass surface as notifications, with transparent
corners and native rounded buttons. Errors preserve the draft and reuse the same request ID on retry,
so an uncertain response cannot create duplicates. Changing title, project, or
placement starts a new request. Successful creation clears the draft and resets
placement to the top.

Paste a screenshot with **⌘V** or **Paste image**. Copied image files also work.
Up to eight images, each at most 10 MiB, appear in a horizontally scrolling preview
strip; each has a remove button. Ordinary text pastes still edit the title. Images
stay with a dismissed draft and become ordinary issue attachments on creation.
Image submissions first save a non-runnable draft, upload every image, then publish
the issue. On failure, **Retry** resumes the same submission with editing locked,
so workers never receive an issue missing its images. If the app exits mid-upload,
the saved issue remains a draft in the issue list. Unsubmitted in-memory images do
not survive app exit. Temporary upload files are removed after success or exit.

The panel calls the installed Rust CLI against this machine's normal issue store.
Fleet companions keep their usual durable local replicas and synchronization, so
issue creation works without a browser or a live supervisor connection. Linux
companions receive the updated CLI; the native panel and shortcut require macOS.

Run the focused native regression audit with `HEY_BOSS_AUDIT_QUICK_ISSUE_ONLY=1`
after compiling `hey_boss_daemon.swift` and `test_hey_boss.swift` with
`-parse-as-library -D HEY_BOSS_AUDIT`. Optionally set
`HEY_BOSS_QUICK_ISSUE_SCREENSHOTS` to a disposable directory for light/dark renders
of empty, loading, project-picker, validation-error, long-title, saving, and success
states, including an assertion that the outer corners are transparent.
Window captures use macOS `screencapture` and need existing Screen Recording access;
ordinary audits do not capture the screen. For interactive
testing set `HEY_BOSS_NATIVE_QUICK_ISSUE_PREVIEW=1` and point
`HEY_BOSS_CLI_PATH` and `HEY_BOSS_ISSUE_DB` at an updated CLI and isolated test store.
`HEY_BOSS_QUICK_ISSUE_INTEGRATION=1` adds real CLI/attachment-store checks (requires
an isolated `HEY_BOSS_ISSUE_DB` under `/tmp/`). For visual inspection without Screen
Recording access, launch the audit app with `HEY_BOSS_QUICK_ISSUE_VISUAL` set to a
fixture such as `light-images`, `dark-saving`, `light-error`,
`dark-empty`, `light-success`, or `light-images-compact`.
Set `HEY_BOSS_QUICK_ISSUE_RENDERS` to render the same native controls on a plain
surface without capturing the desktop; this checks layout but not live glass.
