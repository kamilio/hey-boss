# Updating instructions during a task

Save edits in Project settings to send updated instructions to running issue
agents. Workers check for changes every two seconds. Connected fleet devices
receive the same behavior once the settings reach their local replica; offline
devices apply the latest instructions after reconnecting.

The update goes to the agent's existing session and active turn. Its progress,
issue claim, workspace choice, delivery mode, and goal lifecycle are retained.
If its turn finishes before it accepts steering, the worker starts a follow-up
turn in the same session before accepting its completion report. Rejected
steering is logged and retried without restarting the agent.

Only instructions the task actually uses are sent. Worker-specific prompt
overrides take precedence over project settings. Edits to inactive workflow
branches do not send an update. Plan and research tasks continue to use their
artifact instructions. Non-prompt settings do not send steering. Several edits
between checks are combined into the latest instructions.

Agents already running an older worker build must finish or be restarted once
after upgrading to enable this behavior. Subsequent prompt edits need no restart.
The saved run prompt and worker activity show acknowledged updates; settings
saving confirms persistence, not that every disconnected agent has received them.
