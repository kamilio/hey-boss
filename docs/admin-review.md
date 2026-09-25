# Agent review pane

Open `/admin` in the local issue web service, or choose **Review** in its navigation.
The page is read-only. It shows all built-in prompt Markdown, a complete worker
prompt for a selected real project/issue, the canonical skill, and the web page
discovery guide. Character and line counts make large inputs easy to spot.
The full worker preview uses the existing preview operation, including project
overrides, template expansion, workflow branches, and handoff context.

The command catalog comes from Clap, including nested commands, aliases, help,
and JSON option support. Search filters names, descriptions, and prompt content.
Text and JSON appear side by side, with the invocation, exit status, stderr, and
copy controls. Nothing is stored in browser storage.

Issue, artifact, attachment, and mindmap samples use private temporary databases.
The selected real issue's title and body are copied into the sample; its live
database is never modified. Each output format starts with fresh sample state,
so a mutation in the text preview cannot change the JSON preview. Notification
examples use a sample daemon response and the production CLI formatter; they do
not send a notification or ask a question. Skill installation examples write
only inside the temporary preview directory.

Commands that need external services, process control, provisioning, or an
interactive UI show help and an explicit explanation instead of executing.
The catalog marks these **HELP**, while supported output samples show **OUTPUT**.
There is no arbitrary command/argument execution endpoint. Captures are serialized,
time limited, capped at 1 MiB per output stream, and removed after the response.

The CLI exposes the same catalog and samples:

```sh
hey-boss admin catalog --json
hey-boss admin preview 'issue view' --json
hey-boss admin preview 'artifact create' --json
hey-boss skill show
hey-boss skill show --json
hey-boss skill install
hey-boss skill install --json
```

`skill install` installs the bundled `skills/hey-boss/SKILL.md` in the current
user's `.codex`, `.agents`, and `.claude` skill directories, leaving unrelated
skills intact. Prompt and skill sources are compiled into the binary; source
edits require rebuilding and updating the service. The review page displays the
running build ID, so the preview's version is explicit.

The page is served locally; the mobile navigation does not advertise it.
