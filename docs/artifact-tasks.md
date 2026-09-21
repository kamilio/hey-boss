# Plan tasks

Choose **Task → Plan** in the issue editor, Quick Add, or phone issue form.
Implement is the default. A Plan badge identifies artifact tasks; readiness
and draft state remain independent of task intent.

Edit **Project settings → Plan → Plan prompt** to customize what planning
agents receive. The built-in prompt asks for scope, design, tradeoffs,
implementation steps, and verification criteria, saved as linked artifacts.
**Use default** clears the project override. Saving prompt edits also updates
running agents that inherit this prompt, including connected devices.

The Plan prompt replaces implementation, workspace, and delivery instructions,
including when the project normally uses worktrees or PRs. It supports the same
placeholders as implementation prompts. Start it with `/goal` to enable goal
mode; goal mode also follows the implementation prompt's `/goal` setting.
Select **Preview task → Plan** to inspect the assembled instructions without
changing any issues or worker settings.

By default, agents can organize related artifacts in a mindmap and create draft
follow-up issues without starting implementation. When drafts are disabled,
proposed follow-ups stay in the artifact. Successful Plan tasks close normally
without a PR handoff.

Task intent uses the durable `task:plan` label:

```sh
hey-boss issue create --title 'Plan reconnect improvements' --label task:plan
```

Research is no longer a separate task. Older `task:research` issues use the Plan
prompt and appear as Plan tasks. Editing their task choice replaces the old
label with `task:plan`, preserving unrelated labels. Pending phone submissions
retain their original payload and request ID so retries remain idempotent.
Ordinary `plan` and `research` labels do not affect task behavior. Changing a
Plan task to Implement during an active run prevents stale completion from
closing the changed task.
