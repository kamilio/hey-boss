# Open Issues from anywhere

Press **Command+Control+Option+Shift+O** anywhere on macOS to open Issues in
your default browser. This uses the same modifiers as Quick Add, with **O**
for Open. The shortcut is also displayed beside **Issues…** in the Hey Boss
menu bar.

The desktop app must be running. It reuses the local issue service on port
4781, or starts it with the installed CLI when needed. Repeated activation
while the service starts does not launch extra servers. Startup and browser
errors use the existing Issues error dialog. Global registration needs no
Accessibility permission; if another application owns the shortcut, the
menu remains available.

The shortcut is registered for the desktop app's lifetime and released on exit.
SSH companions receive the normal software update; the native shortcut is a
macOS desktop feature.

Run the focused native audit by compiling `hey_boss_daemon.swift` and
`test_hey_boss.swift` with `-parse-as-library -D HEY_BOSS_AUDIT`, then running
with `HEY_BOSS_AUDIT_ISSUES_SHORTCUT_ONLY=1`.
