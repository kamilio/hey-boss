# hey-t3

Use a T3 thread link from the terminal, even with Desktop closed. The Hey Talk installer adds `hey-t3` to `~/.local/bin`; you can also run `./hey-t3.mjs` directly. Requires Node 22.15 or newer, a running T3 server on the thread’s host, and a connected Hey Boss companion for remote threads.

```sh
hey-t3 projects                # Projects on this Mac and connected machines
hey-t3 project automations     # Find the project automatically
hey-t3 list                    # Threads across connected machines, with usable links
hey-t3 list --project automations
hey-t3 show "$link"            # Thread metadata and the whole conversation in order
hey-t3 pull "$link" > thread.json  # Full native transcript, including tool activity
hey-t3 catch-up "$link"        # Your last message and the assistant replies since
hey-t3 send "$link" "$message"     # Start now or steer the active turn
hey-t3 queue "$link" "$message"    # Next turn if busy; starts if idle
hey-t3 rename "$link" 'Release notes'
hey-t3 pin "$link"             # Also: unpin, settle
```

Use `-` instead of the message to read from stdin. Both commands return a message ID and the observed delivery. T3 may queue a send if the active provider cannot steer. `pull` uses T3’s native transcript export (50 MiB server limit).

A thread link identifies its environment. For remote threads, the CLI matches that ID against machines already connected in Hey Boss. It does not open a separate login or need its own host list. T3 credentials stay on the owning machine.

Project lookup and launch use the first match: this Mac first, then connected machines in stable order. Use a project ID to choose another match, or `--env` to limit a command to one environment. Listing and search skip unavailable machines and report those failures on stderr.

Authentication uses the running server's version. If a desktop update has replaced its CLI, hey-t3 downloads the matching official release, verifies its checksum, and caches it under `~/.t3/caches/hey-t3/`. Active threads keep running; no version setting or server restart is needed.

Use `hey-t3 start PROJECT [MESSAGE]` to launch with the project's or environment's configured model. If neither has one, hey-t3 uses Codex GPT-6.1-Sol. Use `--model` to override it, or `--worktree BASE` to start in a worktree. Run `hey-t3 --help` for project lookup, search, queue promotion, stopping, and scheduling.
