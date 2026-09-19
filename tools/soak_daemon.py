#!/usr/bin/env python3
"""Exercise an isolated launchd daemon without displaying notifications or using SSH."""
import argparse
import concurrent.futures
import datetime
import json
import hashlib
import os
from pathlib import Path
import plistlib
import re
import socket
import sqlite3
import subprocess
import time
import uuid

LABEL = 'local.hey-boss-reliability-test'

def command(*args):
    return subprocess.run(args, capture_output=True, text=True)

def call(path, payload):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(15)
        client.connect(str(path))
        client.sendall(payload if isinstance(payload, bytes) else json.dumps(payload).encode())
        client.shutdown(socket.SHUT_WR)
        chunks = []
        while chunk := client.recv(8192):
            chunks.append(chunk)
            if sum(map(len, chunks)) > 1024 * 1024:
                raise RuntimeError('Unbounded daemon response')
        return json.loads(b''.join(chunks))

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--daemon', required=True, type=Path)
    parser.add_argument('--cli', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--until', required=True, help='UTC ISO timestamp')
    parser.add_argument('--label', default=LABEL, help='Isolated reliability LaunchAgent label')
    args = parser.parse_args()
    if not re.fullmatch(r'local\.hey-boss-reliability-[a-z0-9-]+', args.label):
        raise RuntimeError('Label must stay in the isolated reliability namespace')
    until = datetime.datetime.fromisoformat(args.until.replace('Z', '+00:00')).timestamp()
    if until <= time.time():
        raise RuntimeError('Soak deadline must be in the future')
    os.umask(0o077)
    run = args.output.resolve() / str(uuid.uuid4())
    run.mkdir(parents=True)
    domain = f'gui/{os.getuid()}/{args.label}'
    if command('launchctl', 'print', domain).returncode == 0:
        raise RuntimeError('An isolated reliability service is already loaded')
    path = run / 'daemon.sock'
    if len(os.fsencode(path)) >= 104:
        raise RuntimeError('Test socket path exceeds macOS Unix-socket limit; use a shorter output directory')
    # Completed synthetic records never restore cards or display questions.
    with sqlite3.connect(run / 'history.db') as database:
        database.execute('CREATE TABLE dialogs (id TEXT PRIMARY KEY, status TEXT NOT NULL, body TEXT NOT NULL)')
        for task_id, kind, status, result in [('known', 'alert', 'ok', None), ('answer', 'prompt', 'ok', 'Synthetic answer'), ('cancelled', 'prompt', 'cancelled', None)]:
            record = dict(taskID=task_id, kind=kind, question='Synthetic', project='Reliability test', title='Synthetic', description='', options=[], createdAt=time.time(), status=status, result=result)
            database.execute('INSERT INTO dialogs VALUES (?, ?, ?)', (task_id, status, json.dumps(record)))
    config = dict(Label=args.label, ProgramArguments=[str(args.daemon.resolve())],
                  EnvironmentVariables=dict(HEY_BOSS_STATE_DIR=str(run), HEY_BOSS_CLI_PATH=str(args.cli.resolve())),
                  LimitLoadToSessionType='Aqua', ProcessType='Interactive',
                  Sockets=dict(Listener=dict(SockPathName=str(path), SockPathMode=0o600, SockType='stream')),
                  StandardOutPath=str(run / 'daemon.log'), StandardErrorPath=str(run / 'daemon.log'))
    plist = run / 'service.plist'
    plist.write_bytes(plistlib.dumps(config))
    loaded = False
    total = failures = 0
    peak_rss = 0
    start = time.time()
    log = run / 'samples.jsonl'
    manifest = dict(label=args.label, deadline=args.until, started_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    binaries={str(path.resolve()): hashlib.sha256(path.read_bytes()).hexdigest() for path in [args.daemon, args.cli]},
                    harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
    (run / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    def emit(**values):
        sample = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(), **values)
        with log.open('a') as file:
            file.write(json.dumps(sample) + '\n')
        print(json.dumps(sample), flush=True)
    try:
        result = command('launchctl', 'bootstrap', f'gui/{os.getuid()}', str(plist))
        if result.returncode:
            raise RuntimeError(result.stderr)
        loaded = True
        while not path.exists():
            if time.time() - start > 10:
                raise RuntimeError('Test socket did not appear')
            time.sleep(.1)
        cases = [
            (dict(command='status', task_id='known', sync=False), 'ok'),
            (dict(command='wait', task_id='answer', sync=False), 'ok'),
            (dict(command='wait', task_id='cancelled', sync=False), 'cancelled'),
            (dict(command='hide', task_id='known', sync=False), 'ok'),
            (dict(command='unknown', sync=False), 'error'),
            (dict(command='status', task_id='missing', sync=False), 'error'),
            (dict(command='ask', sync=False), 'error'),
            (dict(command='update', project='Test', title='Test', question='Test', description='', link_url='https://example.com', sync=False), 'error'),
            (dict(command='alert', project='Test', title='Test', question='Test', autoclose=-1, sync=False), 'error'),
            (b'{broken json', 'error'), (b'', 'error'),
        ]
        def exercise(case):
            payload, expected = case
            response = call(path, payload)
            if response.get('status') != expected:
                raise RuntimeError(f'Unexpected status: {response}')
            if isinstance(payload, dict) and payload.get('task_id') == 'answer' and response.get('result') != 'Synthetic answer':
                raise RuntimeError('Answer changed')
            if expected == 'cancelled' and response.get('result') is not None:
                raise RuntimeError('Cancellation returned an answer')
        emit(event='started', state=str(run), deadline=args.until)
        next_sample = 0
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            while time.time() < until:
                for future in [pool.submit(exercise, case) for case in cases]:
                    total += 1
                    try:
                        future.result()
                    except Exception as error:
                        failures += 1
                        emit(event='request_failure', error=str(error), requests=total)
                        raise
                if time.time() >= next_sample:
                    service = command('launchctl', 'print', domain)
                    match = re.search(r'\bpid = (\d+)', service.stdout)
                    if service.returncode or not match:
                        raise RuntimeError('Test daemon is no longer running')
                    pid = match.group(1)
                    process = command('ps', '-p', pid, '-o', 'rss=')
                    rss = int(process.stdout.strip())
                    peak_rss = max(peak_rss, rss)
                    if rss > 512 * 1024:
                        raise RuntimeError('Test daemon RSS exceeded 512 MiB')
                    emit(event='sample', pid=int(pid), requests=total, failures=failures, rss_kib=rss, peak_rss_kib=peak_rss, elapsed_seconds=round(time.time()-start))
                    next_sample = time.time() + 60
                time.sleep(min(5, max(0, until-time.time())))
        emit(event='passed', requests=total, failures=failures, peak_rss_kib=peak_rss, elapsed_seconds=round(time.time()-start))
    except BaseException as error:
        emit(event='failed', error=str(error), requests=total, failures=failures)
        raise
    finally:
        if loaded:
            result = command('launchctl', 'bootout', domain)
            emit(event='service_stopped', returncode=result.returncode)

if __name__ == '__main__':
    main()
