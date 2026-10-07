#!/usr/bin/env python3
"""Opt-in real Claude Code + Gemini Messages task/auto-compaction regression.

Requires an already running standalone proxy and the Claude CLI. Makes real model
calls. Run: python3 tests/messages_claude_live.py --base-url http://127.0.0.1:8080/custom
Artifacts stay in the printed temporary directory. No user Claude settings change.
"""
import argparse
import csv
from decimal import Decimal
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--base-url', default='http://127.0.0.1:8080/custom')
parser.add_argument('--model', default='gemini-test')
parser.add_argument('--claude', default='claude')
args = parser.parse_args()
root = Path(tempfile.mkdtemp(prefix='hey-proxy-claude-'))
work = root / 'task'
work.mkdir()
print(f'Live test artifacts: {root}', flush=True)
env = os.environ.copy()
for key in ('ANTHROPIC_AUTH_TOKEN', 'CLAUDE_CODE_USE_BEDROCK', 'CLAUDE_CODE_USE_VERTEX',
            'CLAUDE_CODE_USE_FOUNDRY', 'DISABLE_AUTO_COMPACT', 'DISABLE_COMPACT'):
    env.pop(key, None)
env.update(ANTHROPIC_BASE_URL=args.base_url, ANTHROPIC_API_KEY='hey-proxy',
           ANTHROPIC_MODEL=args.model, ANTHROPIC_DEFAULT_HAIKU_MODEL=args.model,
           ANTHROPIC_DEFAULT_SONNET_MODEL=args.model, ANTHROPIC_DEFAULT_OPUS_MODEL=args.model,
           CLAUDE_CONFIG_DIR=str(root / 'claude'), DISABLE_NON_ESSENTIAL_MODEL_CALLS='1',
           CLAUDE_CODE_AUTO_COMPACT_WINDOW='100000', CLAUDE_AUTOCOMPACT_PCT_OVERRIDE='20')
common = [args.claude, '--model', args.model, '--tools', 'Read,Edit,Write,Bash',
          '--allowedTools', 'Read,Edit,Write,Bash', '--safe-mode', '--setting-sources', '',
          '--strict-mcp-config', '--output-format', 'stream-json', '--verbose']

def run(name, prompt, session=None):
    command = common + ['-p', prompt]
    if session:
        command += ['--resume', session]
    path = root / f'{name}.jsonl'
    with path.open('w') as output, (root / f'{name}.err').open('w') as error:
        subprocess.run(command, cwd=work, env=env, stdout=output, stderr=error,
                       timeout=600, check=True)
    events = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    results = [e for e in events if e.get('type') == 'result']
    assert results and not results[-1].get('is_error'), f'{name}: Claude failed; see {path}'
    assert not results[-1].get('permission_denials'), f'{name}: tools denied'
    print(f'{name}: Claude completed', flush=True)
    return results[-1]['session_id'], events

# Claude can hide an SSE adapter failure by retrying without streaming. Check
# the wire directly first so successful coding cannot mask a broken stream.
payload = dict(model=args.model, max_tokens=8192, stream=True,
               thinking=dict(type='adaptive'),
               messages=[dict(role='user', content='Reply with STREAM_OK only.')])
request = urllib.request.Request(args.base_url.rstrip('/') + '/v1/messages',
                                data=json.dumps(payload).encode(),
                                headers={'Content-Type': 'application/json',
                                         'x-api-key': 'hey-proxy',
                                         'anthropic-version': '2023-06-01'})
with urllib.request.urlopen(request, timeout=120) as response:
    assert 'text/event-stream' in response.headers.get('Content-Type', '')
    wire = response.read().decode()
(root / 'stream-preflight.sse').write_text(wire)
stream_events = [json.loads(line[5:]) for line in wire.splitlines() if line.startswith('data:')]
assert not any(e.get('type') == 'error' for e in stream_events), 'SSE preflight returned an error'
assert stream_events[-1]['type'] == 'message_stop', 'SSE preflight did not complete'
assert any(e.get('type') == 'message_delta' and e.get('usage', {}).get('input_tokens', 0) + e.get('usage', {}).get('cache_read_input_tokens', 0) > 0 for e in stream_events), 'SSE preflight lacks real usage'
assert 'STREAM_OK' in ''.join(e.get('delta', {}).get('text', '') for e in stream_events), 'Unexpected streaming reply'
print('Streaming preflight: passed without Claude fallback', flush=True)

(work / 'invoice.py').write_text('def total(rows):\n    return sum(float(r["price"]) * r["quantity"] for r in rows)\n')
session, events = run('invoice', 'Work only in this directory. Read invoice.py, fix total to use Decimal arithmetic and reject negative prices and quantities. Add test_invoice.py with unittest tests for 0.1 + 0.2, empty rows, quantities, and negative cases. Run python3 -m unittest -v and fix failures.')
subprocess.run(['python3', '-m', 'unittest', '-v'], cwd=work, check=True)
# Verify behavior independently of model-written tests.
subprocess.run(['python3', '-c', 'from invoice import total; from decimal import Decimal; assert total([{"price":"0.1","quantity":1},{"price":"0.2","quantity":1}]) == Decimal("0.3"); assert total([]) == Decimal(0)'], cwd=work, check=True)

rng = random.Random(4187)
expected = {}
for filename in ('ledger_a.csv', 'ledger_b.csv'):
    rows, total, invalid = [], Decimal(0), 0
    for index in range(700):
        price, quantity = Decimal(rng.randrange(1, 100000)) / 100, rng.randrange(1, 9)
        if index in (5, 257):
            price = -price
        if index == 499:
            quantity = -quantity
        rows.append(dict(id=f'item-{index:04d}', price=str(price), quantity=quantity,
                         reference=f'{rng.getrandbits(48):012x}'))
        if price < 0 or quantity < 0:
            invalid += 1
        else:
            total += price * quantity
    with (work / filename).open('w') as file:
        writer = csv.DictWriter(file, fieldnames=['id', 'price', 'quantity', 'reference'])
        writer.writeheader()
        writer.writerows(rows)
    expected[filename] = dict(total=f'{total:.2f}', valid_rows=700-invalid, invalid_rows=invalid)
(root / 'expected.json').write_text(json.dumps(expected, indent=2))
session, ledger_events = run('ledger', 'Work only in this directory. First use Read to inspect both ledger_a.csv and ledger_b.csv completely. Implement ledger.py using Decimal to total price times quantity, skipping negative prices/quantities. Add tests and run them. Write ledger_summary.json mapping each filename to total (two-decimal string), valid_rows and invalid_rows, using your program on the full files. Verify the result.', session)
events += ledger_events
# A further coding turn ensures compaction leaves a usable conversation.
_, followup_events = run('continuation', 'Add a --strict CLI option to ledger.py that exits nonzero on a negative price or quantity while retaining default skip-invalid behavior. Support python3 ledger.py [--strict] FILE.csv. Test both modes, run all tests, and regenerate ledger_summary.json in default mode. Work only in this directory.', session)
events += followup_events
subprocess.run(['python3', '-m', 'unittest', '-v'], cwd=work, check=True)
assert json.loads((work / 'ledger_summary.json').read_text()) == expected, 'Ledger totals differ from independent oracle'
for filename in expected:
    default = subprocess.run(['python3', 'ledger.py', filename], cwd=work, capture_output=True)
    strict = subprocess.run(['python3', 'ledger.py', '--strict', filename], cwd=work, capture_output=True)
    assert default.returncode == 0, f'{filename}: default CLI failed'
    assert strict.returncode != 0, f'{filename}: strict CLI accepted negative data'
# Strict mode must accept valid input; rejecting every invocation is not a pass.
(work / 'valid.csv').write_text('id,price,quantity,reference\nvalid,0.10,3,ok\n')
subprocess.run(['python3', 'ledger.py', '--strict', 'valid.csv'], cwd=work, check=True)

boundaries = [i for i, e in enumerate(events) if e.get('type') == 'system' and e.get('subtype') == 'compact_boundary' and e.get('compact_metadata', {}).get('trigger') == 'auto']
assert boundaries, 'No successful automatic compaction boundary; a compacting status alone is insufficient'
assert any(b.get('type') == 'tool_use' for e in events[boundaries[-1]+1:] if e.get('type') == 'assistant' for b in e.get('message', {}).get('content', [])), 'No real tool use after compaction'
failures = [e for e in events if e.get('compact_result') == 'failed' and e.get('compact_error') != 'too_few_groups']
assert not failures, f'Compaction failures: {failures}'
print(json.dumps(dict(result='passed', compactions=len(boundaries), artifacts=str(root), totals=expected), indent=2))
