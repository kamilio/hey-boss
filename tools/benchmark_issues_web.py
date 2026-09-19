#!/usr/bin/env python3
"""Read-only HTTP timings against the synthetic issue web review fixtures."""
import argparse
import json
import math
import statistics
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--url', default='http://127.0.0.1:4782')
parser.add_argument('--samples', type=int, default=60)
args = parser.parse_args()
base = args.url.rstrip('/')
bootstrap = json.load(urllib.request.urlopen(base + '/api/bootstrap', timeout=10))
token = bootstrap['csrf']

def operation(project, value):
    payload = json.dumps({'project': project, 'operation': value, 'request_id': None}).encode()
    request = urllib.request.Request(base + '/api/action', data=payload, headers={
        'Content-Type': 'application/json', 'X-Hey-Boss-CSRF': token})
    start = time.perf_counter()
    with urllib.request.urlopen(request, timeout=10) as response:
        body = response.read()
    elapsed = (time.perf_counter() - start) * 1000
    result = json.loads(body)
    assert result['ok'], result
    return elapsed, len(body), result

listing = {'action': 'list', 'state': 'open', 'mine': False, 'unassigned': False,
           'labels': [], 'search': None, 'limit': 50, 'offset': 0, 'all': True}
cases = [
    ('project_switcher', 'github.com/example/hey-boss', {'action': 'projects'}),
    ('small_project_list', 'github.com/example/hey-boss', listing),
    ('5000_issue_list', 'named:Scale test', listing),
    ('5000_issue_search', 'named:Scale test', dict(listing, search='fixture 499')),
    ('issue_detail', 'github.com/example/hey-boss', {'action': 'view', 'number': 1}),
]
output = {'samples': args.samples, 'measurements': {}, 'assets': {}}
for name, project, op in cases:
    operation(project, op)
    samples = [operation(project, op) for _ in range(args.samples)]
    values = sorted(item[0] for item in samples)
    output['measurements'][name] = {
        'median_ms': round(statistics.median(values), 2),
        'p95_ms': round(values[math.ceil(len(values) * .95) - 1], 2),
        'max_ms': round(max(values), 2),
        'response_bytes': samples[-1][1],
    }
    if name == '5000_issue_list':
        assert len(samples[-1][2]['issues']) == 5000
        assert all('body' not in row for row in samples[-1][2]['issues'])
for path in ('/', '/app.js', '/app.css', '/icon.png'):
    with urllib.request.urlopen(base + path, timeout=10) as response:
        output['assets'][path] = len(response.read())
output['total_asset_bytes'] = sum(output['assets'].values())
print(json.dumps(output, indent=2))
