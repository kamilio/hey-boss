#!/usr/bin/env python3
"""Exercise an isolated mindmap server during repeated CLI and Inbox changes."""
import argparse
import json
import os
import pathlib
import shutil
import socket
import sqlite3
import subprocess
import threading
import time
import urllib.error
import urllib.request
import urllib.parse


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--seconds', type=int, default=300)
    args = parser.parse_args()
    checkout = pathlib.Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    if checkout / 'out' not in output.parents or args.seconds < 1:
        parser.error('Use a new directory under checkout out/ and a positive duration')
    output.mkdir(parents=True, exist_ok=False)
    source_cli = args.cli.resolve(strict=True)
    # Concurrent local installs trigger the server's executable reload watcher.
    # Pin both authoring and serving to one build for this endurance measurement.
    cli = output / 'hey-boss'
    shutil.copy2(source_cli, cli)
    database = output / 'issues.db'
    inbox_path = output / 'inbox.sock'
    env = dict(os.environ, HEY_BOSS_ISSUE_DB=str(database), HEY_BOSS_INBOX_SOCKET=str(inbox_path))
    for key in ['HEY_BOSS_ISSUE_HOST', 'HEY_BOSS_ISSUE_PROJECT']:
        env.pop(key, None)
    state = {'pending': True, 'available': True}
    stop = threading.Event()
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(inbox_path))
    listener.listen()
    listener.settimeout(.2)

    def inbox():
        while not stop.is_set():
            try:
                client, _ = listener.accept()
            except socket.timeout:
                continue
            with client:
                client.settimeout(10)
                raw = bytearray()
                while chunk := client.recv(65536):
                    raw.extend(chunk)
                request = json.loads(raw)
                with (output / 'inbox-requests.jsonl').open('a') as log:
                    log.write(json.dumps(request) + '\n')
                if request['command'] != 'inbox_list' or not state['available']:
                    reply = {'status': 'error', 'error': 'Synthetic snapshot unavailable'}
                else:
                    notice = {'taskID': 'review', 'title': 'Review release', 'summary': 'Check dependencies', 'status': 'pending' if state['pending'] else 'read'}
                    reply = {'status': 'ok', 'result': json.dumps({'tasks': [notice]})}
                client.sendall(json.dumps(reply).encode())

    thread = threading.Thread(target=inbox)
    thread.start()
    server = None
    started = time.monotonic()
    rounds = 0

    def command(group, project, *words):
        result = subprocess.run([str(cli), group, '--json', '--agent', 'human:soak', '--project', project, *words], env=env, cwd=output, capture_output=True, timeout=25, check=True)
        value = json.loads(result.stdout)
        assert value['ok'], value
        return value

    def sample(event, **fields):
        value = {'event': event, 'rounds': rounds, 'elapsed_seconds': round(time.monotonic() - started, 2), **fields}
        with (output / 'samples.jsonl').open('a') as log:
            log.write(json.dumps(value) + '\n')
        print(json.dumps(value), flush=True)

    try:
        command('mm', 'Atlas', 'add', 'Release', '--id', 'release')
        command('mm', 'Atlas', 'add', 'Planning', '--under', 'release', '--id', 'plan')
        command('issue', 'Atlas', 'create', '--title', 'Implementation', '--body', 'Live issue')
        command('issue', 'Atlas', 'pr', 'add', '1', 'https://github.com/example/repo/pull/1')
        command('mm', 'Atlas', 'issue', '1', '--under', 'release', '--id', 'implementation')
        command('mm', 'Platform', 'add', 'Shared API', '--id', 'api')
        command('mm', 'Atlas', 'notice', 'review', '--under', 'release', '--id', 'review')
        command('mm', 'Atlas', 'link', 'review', 'implementation')
        command('mm', 'Atlas', 'link', 'pr:https://github.com/example/repo/pull/2', 'pr:https://github.com/example/repo/pull/1', '--kind', 'depends-on')
        with (output / 'server.log').open('w') as log:
            server = subprocess.Popen([str(cli), 'mm', '--project', 'Atlas', '--agent', 'human:soak', '--json', 'web', '--port', '0', '--no-discovery'], env=env, cwd=output, stdout=log, stderr=log)
        deadline = time.monotonic() + 20
        while True:
            try:
                announced = urllib.parse.urlsplit(json.loads((output / 'server.log').read_text().splitlines()[0])['url'])
                url = f'{announced.scheme}://{announced.netloc}'
                break
            except (IndexError, ValueError):
                assert server.poll() is None, 'Server exited during startup'
                assert time.monotonic() < deadline, 'Server startup timed out'
                time.sleep(.1)

        def http(path, payload=None, expected=200, reconnect=True):
            nonlocal token
            request = urllib.request.Request(url + path, data=None if payload is None else json.dumps(payload).encode(), headers={'Content-Type': 'application/json', 'X-Hey-Boss-CSRF': token})
            try:
                with urllib.request.urlopen(request, timeout=25) as reply:
                    status, raw = reply.status, reply.read()
            except urllib.error.HTTPError as error:
                status, raw = error.code, error.read()
            if status == 403 and expected == 200 and reconnect and payload is not None:
                operation = payload.get('operation', {}).get('operation', {})
                if operation.get('command') in ['show', 'view', 'links', 'projects']:
                    token = http('/api/bootstrap')['csrf']
                    sample('reconnected')
                    return http(path, payload, expected, reconnect=False)
            assert status == expected, (path, status, raw[:500])
            return json.loads(raw)

        token = ''
        token = http('/api/bootstrap')['csrf']
        sample('started', url=url, source_cli=str(source_cli), pinned_cli=str(cli), build=subprocess.check_output([str(cli), '--version'], text=True).strip())
        next_sample = started
        while time.monotonic() - started < args.seconds:
            rounds += 1
            assert server.poll() is None, 'Server exited during read/edit cycles'
            body = f'## Round {rounds}\n\n' + 'Unicode planning context 🧭 café. ' * 2000
            command('mm', 'Atlas', 'edit', 'plan', '--body', body)
            why = f'API must land first (round {rounds})'
            command('mm', 'Atlas', 'link', 'implementation', 'Platform::api', '--kind', 'depends-on', '--why', why)
            command('issue', 'Atlas', 'edit', '1', '--title', f'Implementation round {rounds}')
            state.update(pending=rounds % 2 == 0, available=rounds % 5 != 0)
            if rounds == 8:
                token = 'synthetic-stale-token'
                sample('token_recovery_probe')
            request = lambda operation: {'project': 'named:Atlas', 'operation': {'action': 'mindmap', 'operation': operation}, 'request_id': None}
            graph = http('/api/mm', request({'command': 'show', 'body_mode': 'preview'}))
            by_alias = {node.get('alias'): node for node in graph['nodes']}
            assert by_alias['implementation']['title'] == f'Implementation round {rounds}'
            assert by_alias['plan']['body_truncated'] and len(by_alias['plan']['body']) == 512
            assert ('review' in by_alias) == (state['pending'] and state['available'])
            assert graph['notifications']['available'] == state['available']
            assert any(link['description'] == why for link in graph['links'])
            assert any(link['automatic'] for link in graph['links'])
            full = http('/api/mm', request({'command': 'view', 'node': 'plan'}))
            assert full['node']['body'] == body
            focused = http('/api/mm', request({'command': 'links', 'node': 'implementation'}))
            assert all(node['body'] == '' for node in focused['nodes'] + focused['external_nodes'])
            if rounds % 10 == 0:
                for path in ['/api/mm', '/api/action']:
                    http(path, request({'command': 'alias', 'node': 'plan', 'alias': 'forbidden'}), expected=403)
                assert command('mm', 'Atlas', 'view', 'plan')['node']['alias'] == 'plan'
            if time.monotonic() >= next_sample:
                with sqlite3.connect(database) as db:
                    assert db.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
                    assert not db.execute('PRAGMA foreign_key_check').fetchall()
                rss = int(subprocess.check_output(['ps', '-p', str(server.pid), '-o', 'rss='], text=True).strip())
                sample('sample', rss_kib=rss, nodes=len(graph['nodes']), links=len(graph['links']))
                next_sample = time.monotonic() + 60
            stop.wait(min(2, max(0, args.seconds - (time.monotonic() - started))))
        sample('passed')
    except BaseException as error:
        sample('failed', error=str(error))
        raise
    finally:
        if server and server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
        stop.set()
        thread.join(timeout=12)
        listener.close()
        inbox_path.unlink(missing_ok=True)
        sample('stopped', server_returncode=None if server is None else server.returncode)


if __name__ == '__main__':
    main()
