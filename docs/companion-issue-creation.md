# Companion issue creation

Connected companions refresh their issue-number reservation through the existing
authenticated fleet connection before creating an issue. This also works for
projects with no configured worker. The supervisor reserves a fresh block if the
old block is exhausted or its next usable number is below an existing issue.
Previously reserved blocks remain owned by their original machine, so delayed
offline changes cannot collide with another machine's issues.

Issue creation remains a local transaction: the issue is immediately readable
and replicated through the normal journal. Reserving numbers does not create an
issue, so a lost reservation response can be retried safely. If the fleet is
unreachable, creation can still consume an existing offline reservation. A
project without a reservation must reconnect before it can create issues.

The CLI puts new issues at the front by default, matching the issue editor and
Quick Add. Use `hey-boss issue create --at-bottom --title 'Later work'` to append
work deliberately. `--at-top` remains supported. Explicit API `at_top: false`
continues to append; existing issues keep their order and numbers.
