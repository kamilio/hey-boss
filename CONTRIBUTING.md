Use macOS 15+, Rust, and Xcode command-line tools. Run the Rust and native checks in README.md before opening a pull request. Keep changes focused and include steps to reproduce UI issues. Inspect individual cards, collapsed/expanded stacks, questions, launch details, and Markdown previews after UI changes.

Use `out/` for test data. Never include real notification history, logs, credentials, or screenshots containing private information. The CLI prints plain text; keep its help concise.

Dialogs share backdrop dismissal through `components.js` and the native `cancel` event. Keep draft cleanup and save guards in that handler; use `closedby="none"` when an explicit decision is required.

The CLI terminal walkthroughs run against built binaries. For example,
`cargo run --locked --manifest-path tools/terminal-checks/Cargo.toml -- project target/debug/hey-boss`
exercises project setup at three widths and writes PNGs under `out/terminal-checks/project`.
Use `--help` for the other suites. Add `--serve` to `transactions` to keep its private web fixture open until Ctrl+C.

For browser checks, build the CLI with `cargo build --locked -p hey-boss --bin hey-boss`.
Run `cargo run --locked -p hey-boss --example web-fixture -- artifact-diagrams`
and open the printed URL. Use `--help` in place of the scenario to list fixtures.
Ctrl+C stops the server and removes its test data.
The `local-http` fixture prints its private `root`. Replace `root/hey-boss` to
exercise a server restart; `root/ready.json` records the PID, port URL and start count.

Run `cargo test --locked -p hey-boss --test web_client -- --ignored` to check the
shipped client in headless Chrome. Set `CHROME_BIN` if Chrome is installed elsewhere.
