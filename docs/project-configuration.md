# Private project configuration

The fleet service automatically creates a private working copy for every visible project under `<state>/project-configs/<project-id-slug>-<id-hash>/.hey-boss/` on each machine. No worker or repository checkout is needed. The state directory defaults to `~/Library/Application Support/hey-boss` on macOS and `~/.local/share/hey-boss` on Linux; relocated installations use their existing state directory. Project IDs keep paths stable when projects are renamed; the hash distinguishes similar IDs. Run `hey-boss config log --project PROJECT --json` to see local paths in `working_copies`.

Automatic copies use the project list's visibility rules: legacy discovery entries are excluded. Hiding a project stops its automatic copy syncing or being recreated; existing files are retained. Explicitly registered checkout copies continue syncing.

Run `hey-boss config sync` inside a registered checkout. It creates `.hey-boss/config.yaml`, `prompts/*.md`, and `project.json`, preserving legacy `.hey-boss` identity files. Keep `.hey-boss` in your global Git ignore; already tracked files remain tracked.

The fleet service reconciles registered working copies every two seconds and after restart. Files, CLI, and web edits share the SQLite settings and distributed history. Checkout paths and sync journals stay local. Run `config sync` again for an immediate sync or to register another checkout.

Unchanged default prompt sections remain inherited. Editing a section creates an override; deleting its Markdown file restores the built-in default and recreates the file. Deleting `worker.md` restores the current implementation default. Invalid files leave the last valid settings active. Correct the file to resume sync; local errors appear in Activity.

**Project settings → Activity** shows authors, machines, changed fields, and before/after values. **Undo this change** previews the inverse of that revision's fields against the current settings. **Restore this version** previews a complete snapshot restore. Both create new revisions. Later changes to unrelated fields survive undo.

`hey-boss config log`, `diff REVISION`, `undo REVISION`, and `restore REVISION` expose the same history. See `--help` for options.

Concurrent saves use the existing deterministic last-write-wins replication order. Both revisions remain available, including offline edits. History begins with changes made after this feature is installed. Filesystem saves identify the local account and machine; CLI/API operations can identify the submitting agent. A watcher cannot identify the process that originally edited a file.
