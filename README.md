# hey-boss

Licensed under [MIT](LICENSE).

Native macOS notifications and questions for coding agents. Short updates stack by project; Read update opens a Markdown preview. Rust library and CLI, Swift/AppKit daemon, SQLite history. No Python or Electron.

Install with Homebrew:

```sh
brew install kamilio/tap/hey-boss
```

Requires macOS 26+ and Xcode command-line tools. Homebrew installs Rust as a build dependency and compiles locally; no unsigned installer download is executed. The first notification or question registers the daemon automatically. Use `brew services restart hey-boss` to restart it, or `brew services stop hey-boss` before `brew uninstall hey-boss`. History is preserved.

For installation directly from source, install Rust and Xcode command-line tools. Run from a logged-in desktop session:

```sh
export HEY_BOSS_STATE_DIR="$HOME/Library/Application Support/hey-boss"
export HEY_BOSS_BIN_DIR="$HOME/bin"
export HEY_BOSS_LAUNCH_AGENTS_DIR="$HOME/Library/LaunchAgents"
swift install_hey_boss.swift
export PATH="$HEY_BOSS_BIN_DIR:$PATH"
hey-boss alert --project Atlas --title Build 'Checks passed' --autoclose 10
hey-boss update --project Atlas --title Report 'Review ready' '# Findings'
hey-boss ask --project Atlas --title Format 'Which format?' '' --option PDF --option Markdown --async
hey-boss wait '<task_id>'
hey-boss hide '<task_id>'
```

The installer builds both executables and registers a launch agent. macOS starts the daemon when a command connects. Keep `hey-boss.state` beside the installed CLI; it records the chosen state directory. Re-run the installer to upgrade. `cargo install` alone does not install the daemon.

Three or more notifications form a collapsible project stack. The project × dismisses the stack. Read update/Open dismisses its card and keeps history. Questions support `--sync` and `--async`. Run `hey-boss --help` for concise usage guidance.

To install the agent skill: `mkdir -p ~/.codex/skills && cp -R skills/hey-boss ~/.codex/skills/`.

Build and check: `cargo test --locked && cargo clippy --locked --all-targets -- -D warnings`; then `mkdir -p out && xcrun swiftc -O -parse-as-library -D HEY_BOSS_AUDIT hey_boss_daemon.swift test_hey_boss.swift -o out/hey-boss-test && out/hey-boss-test`. Rust consumers use `hey_boss::Client::new(socket_path)`; `cargo doc --open` lists the API.

History and launch metadata (working directory, Git branch, process ancestry) stay in the chosen state directory. There is no telemetry. Links open in your associated applications. Review notification contents before sharing history; use synthetic data in bug reports.

To uninstall, run `launchctl bootout "gui/$(id -u)" "$HEY_BOSS_LAUNCH_AGENTS_DIR/local.hey-boss.plist"`, then remove that plist, the installed CLI, its adjacent `hey-boss.state`, and the daemon executable. Keep `history.db` if you want your archive.

Maintainers: run `swift release_hey_boss.swift 0.1.0 kamilio/hey-boss`. Upload the generated source archive and `SHA256SUMS` to the matching GitHub release, then copy `out/hey-boss.rb` to `homebrew-tap/Formula/hey-boss.rb`. The manual Release artifacts workflow produces the same files without publishing.
