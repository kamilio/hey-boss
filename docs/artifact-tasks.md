# Planning and research tasks

Choose **Task → Plan** or **Research** in the issue editor or Quick Add. Implement
is the default. The quiet badge in the issue list identifies artifact tasks;
readiness and draft state remain independent of task intent.

Plan asks the agent for scope, design, tradeoffs, steps, and verification criteria.
Research asks for findings, sources, uncertainties, and recommendations. Both
replace the implementation prompt and Git workspace/delivery instructions with
artifact delivery, including when the project normally uses worktrees or PRs.
The `/goal` prefix still enables goal mode.

Agents save and link artifacts to the task. They can organize related output in
a mindmap and create draft follow-up issues, without starting implementation.
When project drafts are disabled, proposed follow-ups stay in the artifact.
Successful artifact tasks close normally; no PR handoff is required.

Task intent uses the existing durable labels: `task:plan` and `task:research`.
This keeps CLI, replication, transfers, and older clients compatible without a
schema change. CLI example:

```sh
hey-boss issue create --title 'Research reconnect strategies' --label task:research
hey-boss issue edit 12 --remove-label task:research --label task:plan
```

The editor replaces the task label when switching modes and preserves all other
labels. If a CLI user supplies both, Research takes precedence. Ordinary `plan`
or `research` labels do not change prompts. Changing task intent during an active
run prevents that run from closing the changed task.

Regression coverage includes prompt replacement, goal mode, PR-enabled worker
completion, intent changes during a run, and immutable offline phone retries.
`tools/artifact_tasks_browser_checks.js` exercises creation, editing, draft
restoration, failed submission retries, keyboard access, and light/dark layouts
at 320, 390, 768, and 1440 pixels. It refuses to modify a project without the
expected isolated test fixture.
