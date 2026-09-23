# Interrupted validation

Exit 0 alone does not prove a task graph completed. Record an interrupted,
cancelled, timed-out, or incompletely reported run as **incomplete** and stop
steps that depend on its success. This includes commit, push, deployment, and
closing an issue when the project's required validation has not been verified.

## Investigation: issue 135

On September 23, 2026, the preserved poe2 issue 28 log showed Turbo 2.9.14 printing
`Shutting down Turborepo tasks...` followed by `18 successful, 20 total`.
The worker's checkpoint reported exit 0 and an enabled commit hook starting;
the hook was stopped with exit 143, with no new commit or push. The log itself
does not record the process exit code. Its private driver already checks task
totals; this change does not replace or duplicate that driver.

An isolated two-package experiment on this Mac independently reproduced the
behavior using that installation's native Darwin ARM64 Turbo 2.9.14 executable,
without the `bunx` or npm JavaScript launcher. Caching was disabled. A short task
completed, then a signal was sent to the owned process group after the slow task
had started. The uninterrupted control completed both tasks.

| Run | Process exit | Successful / expected | JSON execution | Outcome |
| --- | --- | --- | --- | --- |
| Uninterrupted | 0 | 2 / 2 | success 2, failed 0, attempted 2, exitCode 0 | Passed |
| SIGINT | 0 | 1 / 2 | success 1, failed 0, attempted 2, exitCode 0 | Incomplete |
| SIGTERM | 0 | 1 / 2 | success 1, failed 0, attempted 2, exitCode 0 | Incomplete |

The interrupted JSON reports omitted the unfinished task from `tasks`. Checking
that every *reported* task passed, or checking only `execution.exitCode` and
`execution.failed`, would therefore also accept this incomplete run. The npm
launcher was tested separately and also returned 0 after both signals.
Task scheduling can change how many tasks finish before interruption.

These results establish behavior of the inspected installation, not the original
signal source, an upstream source-code cause, or behavior of all Turbo versions.
Hey Boss does not run or certify the worker's external validation commands.

Reproduce with Node and npm on macOS or Linux, supplying the installed native
Turbo executable (for example, `@turbo/darwin-arm64/bin/turbo`):

```sh
node tools/investigate_turbo_interrupt.mjs /absolute/path/to/native/turbo
```

The probe prints the version, executable SHA-256 and fresh per-run evidence,
then removes its temporary workspace and owned processes. Its exit status
indicates whether the experiment ran, not whether interrupted validation passed.
The inspected executable SHA-256 was
`3098002986866012994826425518a08e72ca5a19ea71902d674448a9154b0b6c`.

## Requirements for automated verification

1. Establish the intended task set, including dependencies and filters, before
   running validation. An empty graph is not proof that required checks ran.
2. Keep the original command, tool version, process status and interruption state.
   Propagate pipeline failures; a successful `tee` is not the task runner's status.
3. Require a normal successful exit **and** fresh terminal completion evidence
   for the entire expected graph. For Turbo, request `--summarize`, associate its
   report with this invocation, and check task identities as well as totals.
   Valid cache hits can count under the project's cache policy; do not require
   every task to have executed again or count cache hits twice.
4. Treat missing, malformed, stale or truncated evidence, absent expected tasks,
   unequal totals, cancellation, signals and timeouts as **incomplete**. Explicit
   task failures are **failed**. Neither outcome satisfies a success gate.
5. Report the outcome clearly, for example:
   `incomplete: interrupted; exit=0; successful=18; expected=20`.
   Preserve evidence and rerun required checks before advancing dependent steps.
   Do not disable existing hooks or weaken project gates to finish delivery.

The canonical Hey Boss skill distributes this guidance to installed agents.
Default main and PR delivery prompts also include it, with the same text visible
in project settings and the rendered instruction preview. Authored prompt
overrides remain intact; this is worker guidance, not a new runtime validator or
a claim that existing custom drivers have been fixed automatically.
