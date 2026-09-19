#!/usr/bin/env python3
"""Create isolated, synthetic issue fixtures for browser review and benchmarks."""
import argparse
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import time

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--database', type=Path, default=root / 'out/issues-web-review/issues.db')
parser.add_argument('--binary', type=Path, default=root / 'target/debug/hey-boss')
args = parser.parse_args()
db = args.database.resolve()
if db.exists():
    raise SystemExit(f'Refusing to overwrite existing database: {db}')
db.parent.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, HEY_BOSS_ISSUE_DB=str(db))
env.pop('HEY_BOSS_ISSUE_HOST', None)

def cli(project, actor, *words):
    result = subprocess.run([str(args.binary), 'issue', '--json', '--project', project,
                             '--agent', actor, *words], env=env, check=True, capture_output=True)
    return json.loads(result.stdout)

project = 'github.com/example/hey-boss'
fixtures = [
    ('Reconnect automatically after waking from sleep', ['bug', 'network']),
    ('Bring project issues into the menu bar', ['enhancement']),
    ('Keep notification history across app updates', ['bug']),
    ('Add keyboard navigation to the issue list', ['enhancement', 'accessibility']),
    ('Show which agent is working on an issue', ['enhancement']),
    ('Improve Markdown table rendering on small screens', ['bug', 'mobile']),
    ('Document the remote companion setup', ['documentation']),
    ('Preserve drafts when switching between projects', ['bug']),
    ('Reduce the time it takes to open the inbox', ['performance']),
    ('Add a compact view for completed work', ['enhancement']),
    ('Support restoring a previous agent session', ['needs-review']),
    ('Group notifications by project', ['enhancement']),
    ('Make closed issues searchable', ['enhancement']),
]
body = '''## What’s happening

After the Mac wakes from sleep, the companion connection stays disconnected until the app is restarted. Pending updates should arrive as soon as the connection is available again.

## Expected behavior

- [ ] Detect when the network becomes available
- [ ] Reconnect without restarting the app
- [ ] Deliver pending updates in their original order
- [ ] Keep retries quiet while the Mac is offline

## Reproduction

1. Connect a remote companion.
2. Put the Mac to sleep for a few minutes.
3. Wake the Mac and send an update from the server.

```sh
hey-boss companion status
```

Keep the existing backoff behavior when a connection cannot be established.
'''
for n, (title, labels) in enumerate(fixtures, 1):
    author = 'human:boss' if n == 9 else 'human:alex' if n % 3 == 0 else 'codex:reconnect-session' if n % 2 else 'claude:overview-session'
    words = ['create', '--title', title, '--body', body if n == 1 else f'## Context\n\n{title}.\n\n## Acceptance criteria\n\n- [ ] Implement the change\n- [ ] Verify the behavior\n- [ ] Update the documentation']
    for label in labels:
        words += ['--label', label]
    cli(project, author, *words)
    if n in (1, 4, 5, 9, 11):
        cli(project, author, 'claim', str(n))
    if n in (1, 3, 4, 6, 9):
        cli(project, author, 'comment', str(n), '--body', 'Reproduced this locally. I’m checking the connection lifecycle and will add coverage for wake-from-sleep behavior.' if n == 1 else 'The initial implementation is ready. Checking the remaining edge cases before closing this out.')
    if n in (12, 13):
        cli(project, author, 'close', str(n))
cli(project, 'human:alex', 'comment', '1', '--body', 'Let’s preserve the queue order during reconnect. The **pending updates** should arrive exactly once, even if the connection drops again.')
for name, titles in {
    'toolcraft': ['Add a compact task inspector', 'Improve tool timeout diagnostics', 'Export a session as Markdown'],
    'git-shelf': ['Preview changed files before publishing', 'Restore the last selected worktree', 'Show branch ahead and behind counts'],
}.items():
    for title in titles:
        cli(f'github.com/example/{name}', 'human:alex', 'create', '--title', title, '--body', f'## Task\n\n{title}.', '--label', 'enhancement')
cli('named:Empty project', 'human:alex', 'create', '--title', 'Placeholder', '--body', '')
cli('named:Empty project', 'human:alex', 'delete', '1')
cli('named:Scale test', 'human:alex', 'create', '--title', 'Performance fixture 1', '--body', 'Synthetic performance data.')
now = int(time.time() * 1000)
with sqlite3.connect(db) as conn:
    conn.execute('UPDATE issues SET created_at=?-number*7200000,updated_at=?-number*3600000', (now, now))
    conn.execute('UPDATE comments SET created_at=?+id*600000', (now-1800000,))
    conn.executemany('INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES(?,?,?,?,?,?,?,?,?,?)',
        [('named:Scale test', n, f'Performance fixture {n}', 'Synthetic Markdown body. ' * 160, 'open', 'human:alex', now-n*1000, now, 1, '["performance"]') for n in range(2, 5001)])
    conn.execute('UPDATE projects SET next_number=5001 WHERE id=?', ('named:Scale test',))
print(db)
