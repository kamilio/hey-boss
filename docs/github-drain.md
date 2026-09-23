`hey-boss issue drain-github` moves GitHub issues into the current hey-boss
project. It requires Python 3 and an authenticated GitHub CLI (`gh`). The
authenticated GitHub login supplies the default creator filter; this avoids
guessing a GitHub account from a Git email address.

```sh
hey-boss issue drain-github
hey-boss issue drain-github --author octocat
hey-boss issue drain-github --repo owner/repo --project target-project --state all
hey-boss issue drain-github --all-authors --json
hey-boss issue --host devbox drain-github --repo owner/repo
```

The default source is the checkout's GitHub repository and the default state is
open. `--state closed` and `--state all` preserve the imported issue's closed
state. Pull requests are excluded. `--all-authors` explicitly
disables the creator filter. Source and comment pagination have no fixed issue limit.

Each imported issue keeps its title and labels. Its body contains the original
Markdown, source URL, author, timestamps, assignees, milestone metadata, and every
comment with its author, timestamps, and original URL. Source assignees are recorded
as metadata; the destination starts unassigned. Each GitHub comment also becomes
a discussion comment under the `github-import` actor, with its original author,
timestamps, and URL in the comment text. The body archive is retained for complete
reads and compatibility with earlier imports. Embedded images and attachments
remain links to their original locations. Imports exceeding the destination's 1 MiB
body limit fail without deleting the source.

The command waits for the issue store to acknowledge the copy and reads it back.
It then reads the source and all comments again, compares the snapshots, and checks
the destination once more before issuing GitHub's `deleteIssue` mutation. A failed
copy, verification, or destination transport check prevents deletion. Discussion
comments are verified through the paginated audit trail, including comments beyond
the latest 20 shown by `issue view`.
Deleting GitHub issues requires the relevant repository permissions. A failed or
uncertain deletion leaves the verified destination copy available; retries only
attempt source issues that still exist. GitHub does not provide
conditional issue deletion, so edits racing with the final deletion cannot be
excluded completely.

Import request IDs are derived from GitHub's stable issue node IDs, under a stable
`github-import` actor, and scoped to the destination project. Retrying the same import
reuses its copy even from another Codex session. Comment request IDs include each
GitHub comment's stable ID, so interrupted imports resume without duplicating
comments. If the source changed after a previous
copy, the retry reports a conflict instead of creating a duplicate or deleting the
new content. Resolve the differing copies manually before continuing. Do not change
the destination project when retrying an interrupted move. Global `--request-id`
is rejected because the command generates IDs for each imported issue.

Failures are reported per source issue, remaining issues are attempted, and the
command exits nonzero if any issue failed. JSON output includes source numbers,
URLs, destination numbers for completed moves, and error details.

For direct invocation, `python3 tools/drain_github_issues.py` accepts the same flags;
it uses `hey-boss` on PATH unless `HEY_BOSS_DRAIN_BINARY` points to a specific CLI.

Run focused checks with:

```sh
cargo build --bin hey-boss
HEY_BOSS_TEST_BINARY=target/debug/hey-boss python3 tools/test_drain_github_issues.py
```
