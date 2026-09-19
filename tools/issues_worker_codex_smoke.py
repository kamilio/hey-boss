#!/usr/bin/env python3
"""Run a harmless native Codex/goal smoke test in an isolated Git repository."""
import json
import os
from pathlib import Path
import signal
import subprocess
import time

root = Path(os.environ.get('HEY_BOSS_SMOKE_ROOT', 'out/worker-codex-smoke')).resolve()
root.mkdir()
cwd = root / 'checkout'
cwd.mkdir()
subprocess.run(['git', 'init', '-q', '-b', 'main', str(cwd)], check=True)
subprocess.run(['git', '-C', str(cwd), 'config', 'user.name', 'Worker Smoke Test'], check=True)
subprocess.run(['git', '-C', str(cwd), 'config', 'user.email', 'worker-test@example.invalid'], check=True)
remote = None
if os.environ.get('HEY_BOSS_SMOKE_REMOTE'):
    remote = cwd / '.git' / 'worker-test-remote.git'
    subprocess.run(['git', 'init', '-q', '--bare', str(remote)], check=True)
    subprocess.run(['git', '-C', str(cwd), 'remote', 'add', 'origin', str(remote)], check=True)
cli = str(Path(os.environ.get('HEY_BOSS_SMOKE_CLI', 'target/debug/hey-boss')).resolve())
env = dict(os.environ, HEY_BOSS_ISSUE_DB=str(root / 'issues.db'), PATH=str(Path(cli).parent) + os.pathsep + os.environ['PATH'])
env.pop('HEY_BOSS_ISSUE_HOST', None)
env.pop('HEY_BOSS_CODEX', None)

def run(*args):
    out = subprocess.run([cli, 'issue', '--agent', 'human:worker-smoke', '--json', *args], cwd=cwd, env=env, check=True, capture_output=True)
    return json.loads(out.stdout)

ordered = bool(os.environ.get('HEY_BOSS_SMOKE_ORDER'))
if ordered:
    run('create', '--title', 'Lower priority control', '--body', 'Do not implement: this issue is only a queue-order control.', '--label', 'ready')
created = run('create', '--title', 'Write and verify a greeting', '--body', 'Create hello.txt containing exactly hello followed by a newline. Verify its bytes with Python. Commit only hello.txt. This is an isolated smoke test; do no other work. Do not send notifications or contact unrelated services.', '--label', 'ready')
number = created['issue']['number']
if ordered:
    run('move', str(number), '--before', '1')

run('settings', 'set', '--prompt', '/goal Assign and implement `{{issue_command}}`. {{commit_instruction}}')

with (root / 'scheduler.log').open('w') as log:
    worker = subprocess.Popen([cli, 'worker', '--tag', 'ready'], cwd=cwd, env=env, stdout=log, stderr=log)
    try:
        deadline = time.monotonic() + 150
        control_disabled = False
        while True:
            status = run('worker', 'status')
            if ordered and status['runs'] and not control_disabled:
                assert status['runs'][0]['number'] == number, status
                run('edit', '1', '--remove-label', 'ready')
                control_disabled = True
            selected = next((r for r in status['runs'] if r['number'] == number), None)
            if selected and selected['finished_at']:
                break
            if time.monotonic() > deadline:
                raise RuntimeError(f'Codex smoke timed out: {status}')
            time.sleep(1)
        (root / 'result.json').write_text(json.dumps(status, indent=2))
        assert selected['state'] == 'completed', status
        assert run('view', str(number))['issue']['state'] == 'closed'
        if ordered:
            assert run('view', '1')['issue']['state'] == 'open'
            assert len(status['runs']) == 1, status
        # Codex may choose a separate Git worktree. Verify the committed
        # artifact in the shared repository rather than its checkout location.
        commits = subprocess.check_output(['git', '-C', str(cwd), 'log', '--all', '--format=%H', '--', 'hello.txt'], text=True).splitlines()
        assert commits, 'No greeting commit was created'
        committed = subprocess.check_output(['git', '-C', str(cwd), 'show', commits[0] + ':hello.txt'])
        assert committed == b'hello\n'
        assert subprocess.check_output(['git', '-C', str(cwd), 'ls-tree', '-r', '--name-only', commits[0]], text=True).splitlines() == ['hello.txt']
        if remote:
            assert subprocess.check_output(['git', '--git-dir', str(remote), 'show', 'main:hello.txt']) == b'hello\n'
        print(json.dumps({'completed': True, 'ordered': ordered, 'issue': number, 'goal': selected['goal'], 'session': selected['session_id'], 'run': selected['id']}))
    finally:
        worker.send_signal(signal.SIGTERM)
        try:
            worker.wait(timeout=8)
        except subprocess.TimeoutExpired:
            worker.kill()
            worker.wait()
