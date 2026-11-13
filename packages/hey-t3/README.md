# hey-t3

Use a T3 thread link from the terminal, even with Desktop closed. The Hey Talk installer adds `hey-t3` to `~/.local/bin`; you can also run `./hey-t3.mjs` directly. Requires Node 22.15 or newer, a running T3 server on the thread’s host, and a connected Hey Boss companion for remote threads.

```sh
hey-t3 list                    # Local thread metadata, including a usable link
hey-t3 list ENVIRONMENT_ID | jq -r '.[] | [.title, .link] | @tsv'
hey-t3 show "$link"            # Thread metadata and messages in the current snapshot
hey-t3 catch-up "$link"        # Your last message and the assistant replies since
hey-t3 rename "$link" 'Release notes'
hey-t3 pin "$link"             # Also: unpin, settle
```

A thread link identifies its environment. For remote threads, the CLI matches that ID against machines already connected in Hey Boss. It does not open a separate login or need its own host list. T3 credentials stay on the owning machine.
