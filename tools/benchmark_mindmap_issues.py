#!/usr/bin/env python3
"""Measure omitted/preview bodies on synthetic native issues, never the live DB."""
import argparse
import json
import os
import pathlib
import sqlite3
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, default=pathlib.Path('target/debug/hey-boss'))
    parser.add_argument('--nodes', type=int, default=500)
    args = parser.parse_args()
    if not 1 <= args.nodes <= 10000:
        parser.error('Use 1..10000 nodes')
    cli = args.binary.resolve(strict=True)
    body = '\0' + 'Planning context 🧭 café. ' * 2500
    with tempfile.TemporaryDirectory(prefix='hey-boss-mm-native-benchmark-') as temporary:
        database = pathlib.Path(temporary) / 'issues.db'
        env = dict(os.environ, HEY_BOSS_ISSUE_DB=str(database), HEY_BOSS_INBOX_SOCKET=str(database.parent / 'absent.sock'))
        for key in ['HEY_BOSS_ISSUE_HOST', 'HEY_BOSS_ISSUE_PROJECT']:
            env.pop(key, None)
        base = [str(cli), 'mm', '--project', 'NativeScale', '--agent', 'human:benchmark', '--json']
        subprocess.run(base, env=env, stdout=subprocess.DEVNULL, check=True)
        subprocess.run([str(cli), 'issue', '--project', 'NativeScale', '--agent', 'human:benchmark', 'create', '--title', 'Seed'], env=env, stdout=subprocess.DEVNULL, check=True)
        with sqlite3.connect(database) as db:
            db.execute('UPDATE issues SET body=?', [body])
            for i in range(2, args.nodes + 1):
                db.execute("INSERT INTO issues(project_id,number,title,body,state,assignee,created_by,closed_by,created_at,updated_at,closed_at,deleted_at,version,labels,sort_order) SELECT project_id,?, ?,body,state,assignee,created_by,closed_by,created_at,updated_at,closed_at,deleted_at,version,labels,? FROM issues WHERE number=1", [i, f'Planning issue {i}', i])
            db.executemany('INSERT INTO mindmap_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?,?)', (
                (f'n-native-{i}', 'named:NativeScale', f'issue-{i}', None, i, 'issue', '', '', str(i), 'named:NativeScale', 1, 1)
                for i in range(1, args.nodes + 1)
            ))
        results = {}
        for mode in ['none', 'preview']:
            times = []
            for _ in range(5):
                started = time.monotonic()
                raw = subprocess.check_output([*base, 'show', '--bodies', mode], env=env)
                times.append(time.monotonic() - started)
                value = json.loads(raw)
                assert len(value['nodes']) == args.nodes
                assert all(n['has_body'] and n['body_truncated'] for n in value['nodes'])
                assert all(n['body'] == ('' if mode == 'none' else body[:512]) for n in value['nodes'])
            results[mode] = {'seconds': times, 'median_seconds': statistics.median(times), 'response_bytes': len(raw)}
        print(json.dumps({'build': subprocess.check_output([str(cli), '--version'], text=True).strip(), 'nodes': args.nodes, 'body_bytes_per_issue': len(body.encode()), 'modes': results}, indent=2))


if __name__ == '__main__':
    main()
