# hey-t3

Use a T3 thread link from the terminal, even with Desktop closed. The Hey Talk installer adds `hey-t3` to `~/.local/bin`; you can also run `./hey-t3.mjs` directly. Requires Node 22.15 or newer, a running T3 server on the thread’s host, and noninteractive SSH for remote hosts.

```sh
hey-t3 connect devbox          # Once per remote host
hey-t3 list                    # Local thread metadata, including a usable link
hey-t3 list ENVIRONMENT_ID | jq -r '.[] | [.title, .link] | @tsv'
hey-t3 show "$link"            # Thread metadata and messages in the current snapshot
hey-t3 catch-up "$link"        # Your last message and the assistant replies since
hey-t3 rename "$link" 'Release notes'
hey-t3 pin "$link"             # Also: unpin, settle
```

`connect` stores the SSH alias under `~/.config/hey-t3/hosts/ENVIRONMENT_ID`. Each command checks the environment ID on the host before accessing a thread. Remote commands run there over SSH; T3 credentials stay on that host. Nothing is installed on the server.
