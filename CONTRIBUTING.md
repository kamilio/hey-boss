Use macOS 15+, Rust, and Xcode command-line tools. Run the Rust and native checks in README.md before opening a pull request. Keep changes focused and include steps to reproduce UI issues. Inspect individual cards, collapsed/expanded stacks, questions, launch details, and Markdown previews after UI changes.

Use `out/` for test data. Never include real notification history, logs, credentials, or screenshots containing private information. The CLI prints plain text; keep its help concise.

Dialogs share backdrop dismissal through `components.js` and the native `cancel` event. Keep draft cleanup and save guards in that handler; use `closedby="none"` when an explicit decision is required.

The CLI terminal walkthroughs run against built binaries. For example,
`cargo run --locked --manifest-path tools/terminal-checks/Cargo.toml -- project target/debug/hey-boss`
exercises project setup at three widths and writes PNGs under `out/terminal-checks/project`.
Use `--help` for the other suites. Add `--serve` to `transactions` to keep its private web fixture open until Ctrl+C.
