#!/usr/bin/env python3
"""A JSONL Codex fixture for scheduler lifecycle and protocol verification."""
import json
from pathlib import Path
import sys
import uuid
import subprocess
import os
import re
import time

session = str(uuid.uuid4())
goal = None
started_turn = False
mode = Path('mode.txt').read_text().strip() if Path('mode.txt').exists() else 'completed'

def send(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    msg = json.loads(line)
    with Path('protocol.jsonl').open('a') as log:
        log.write(json.dumps(msg) + '\n')
    method = msg.get('method')
    if 'id' not in msg or not method:
        continue
    params = msg.get('params', {})
    result = {}
    if method == 'initialize':
        assert params['capabilities']['experimentalApi']
    elif method == 'thread/start':
        assert params['ephemeral'] is False
        Path('.codex-fixture-' + session + '.json').write_text(json.dumps({'turns': []}))
        result = {'thread': {'id': session}}
    elif method == 'thread/resume':
        if mode == 'resume-unavailable':
            send({'id': msg['id'], 'error': {'code': -32600, 'message': 'Saved session is locked by another writer'}})
            continue
        session = params['threadId']
        saved = json.loads(Path('.codex-fixture-' + session + '.json').read_text())
        assert saved['turns'], 'Resume must restore the previous attempt, not an empty session'
        result = {'thread': {'id': session}}
    elif method == 'thread/goal/set':
        if params.get('status') == 'active':
            assert started_turn, 'Goal must be activated after turn/start to preserve output schema'
        goal = dict(goal or {}, **params)
        result = {'goal': goal}
    elif method == 'thread/goal/get':
        result = {'goal': goal}
    elif method == 'turn/start':
        started_turn = True
        text = params['input'][0]['text']
        saved_path = Path('.codex-fixture-' + session + '.json')
        saved = json.loads(saved_path.read_text())
        saved['turns'].append(text)
        saved_path.write_text(json.dumps(saved))
        if mode == 'offline-updates':
            notification = subprocess.run([os.environ['HEY_BOSS_TEST_CLI'], 'update', '--project', 'Offline worker QA', '--title', 'Issue finished', 'Worker finished without waiting for Mac', text, '--json'], capture_output=True)
            assert notification.returncode == 0, notification.stderr.decode()
            assert json.loads(notification.stdout)['status'] == 'pending'
        match = re.search(r"issue view (\d+)", text)
        if mode not in ('unclaimed', 'delay-unclaimed', 'delay-model-start'):
            assert match, text
            assert os.environ['HEY_BOSS_ISSUE_PROJECT'] == 'named:Worker fixture'
            retrieved = subprocess.run([os.environ['HEY_BOSS_TEST_CLI'], 'issue', '--json', 'view', match[1]], capture_output=True)
            assert retrieved.returncode == 0, retrieved.stderr.decode() + retrieved.stdout.decode()
            assert json.loads(retrieved.stdout)['issue']['number'] == int(match[1])
            result_claim = subprocess.run([os.environ.get('HEY_BOSS_TEST_CLI', 'hey-boss'), 'issue', '--json', '--agent', 'codex:' + session, 'claim', match[1]], capture_output=True)
            assert result_claim.returncode == 0, result_claim.stderr.decode() + result_claim.stdout.decode()
        assert params['threadId'] == session
        assert params['outputSchema']['required'] == ['status', 'summary']
        turn = str(uuid.uuid4())
        result = {'turn': {'id': turn}}
    send({'id': msg['id'], 'result': result})
    if method == 'turn/start':
        if mode == 'disconnect':
            sys.exit(9)
        if mode == 'delay-model-start':
            time.sleep(6)
            send({'method': 'item/started', 'params': {'threadId': session, 'item': {'type': 'reasoning'}}})
            continue
        if mode == 'delay-unclaimed':
            send({'method': 'item/started', 'params': {'threadId': session, 'item': {'type': 'reasoning'}}})
            continue
        if mode == 'delay':
            continue
        if mode == 'approval':
            send({'id': 'approval-1', 'method': 'item/commandExecution/requestApproval', 'params': {'threadId': session, 'command': 'synthetic privileged operation'}})
            continue
        text = json.dumps({'status': 'completed' if mode in ('unclaimed', 'offline-updates', 'subtasks-completed') else mode, 'summary': 'Implemented fixture. Meaningful checks passed.'})
        send({'method': 'item/completed', 'params': {'threadId': session, 'item': {'type': 'agentMessage', 'text': text}}})
        send({'method': 'turn/completed', 'params': {'threadId': session, 'turn': {'id': turn, 'status': 'completed'}}})
