const assert = require('node:assert/strict');
const test = require('node:test');
const assignments = require('../src/issues/web/assignments.js');
const helpers = {actorName: id => id === 'human:boss' ? 'Boss' : 'Codex', icon: () => '', bossName: 'Boss'};
const base = {number: 1, state: 'open', version: 1, pull_requests: [{url: 'https://github.com/o/r/pull/1'}]};

test('machine and agent are successive states of the same assignment', () => {
  const queued = assignments.describe({...base, assignment: {kind: 'machine', machine: 'id', machine_name: 'Devbox'}}, helpers);
  assert.equal(queued.label, 'Devbox');
  assert.match(queued.detail, /Waiting for an agent/);
  const running = assignments.describe({...base, assignee: 'codex:123', assignment: {kind: 'agent', actor: 'codex:123', machine: 'id', machine_name: 'Devbox'}}, helpers);
  assert.match(running.label, /Codex/);
  assert.match(running.detail, /Devbox/);
});

test('watcher distinguishes waiting, pending pickup, and active work', () => {
  const describe = extra => assignments.describe({...base, assignment: {kind: 'github', ...extra}}, helpers);
  assert.match(describe({waiting: true}).detail, /Waiting/);
  assert.match(describe({waiting: false}).detail, /queued/);
  assert.match(describe({actor: 'codex:123', machine_name: 'Devbox'}).detail, /Codex.*Devbox/);
  assert.equal(describe({waiting: true}).label, 'GitHub watcher');
});

test('one assignment control preserves Boss and explains when GitHub is unavailable', () => {
  const html = assignments.render({issue: {...base, pull_requests: [], assignment: {kind: 'boss'}}, assignment_machines: [{id: 'box', name: 'Devbox'}]}, helpers);
  assert.match(html, /data-assignment-select/);
  assert.match(html, /value="boss"[^>]*selected/);
  assert.match(html, /value="github"[^>]*disabled/);
  assert.match(html, /Attach a PR/);
  assert.match(html, /value="machine:box"/);
  assert.doesNotMatch(html, /Fleet allocation|Release reservation/);
});

test('watcher destination is unavailable for unsupported or closed pull request links', () => {
  for (const pr of [{url:'https://example.com/o/r/pull/1'}, {url:'https://github.com/o/r/pull/0'}, {url:'https://github.com/o/r/pull/1',status:'closed'}]) {
    const html = assignments.render({issue:{...base,pull_requests:[pr]}},helpers);
    assert.match(html,/value="github"[^>]*disabled/);
  }
});

test('GitHub status and machine names cannot inject markup or links', () => {
  const issue = {...base, assignment: {kind: 'machine', machine: 'box', machine_name: '<img src=x onerror=alert(1)>'}, github_status: {prs: {
    'javascript:alert(1)': {evidence: {head: '123', required: [{context: '<script>bad()</script>', state: 'failure'}], reviews: [{body: '<img onerror=bad()>', html_url: 'javascript:bad()'}]}}
  }}};
  const html = assignments.render({issue, assignment_machines: [{id: 'box', name: '<img onerror=bad()>'}]}, helpers) + assignments.status(issue, helpers);
  assert.doesNotMatch(html, /<img|<script|href="javascript:/);
  assert.match(html, /&lt;/);
});

test('incomplete GitHub evidence is presented without a completion claim', () => {
  const html = assignments.status({...base, assignment: {kind: 'github'}, github_status: {error: 'GitHub unavailable', prs: {}}}, helpers);
  assert.match(html, /Status unavailable/);
  assert.doesNotMatch(html, /All checks finished|Ready to merge/);
});

test('a failed refresh shows its PR error instead of claiming stale checks have finished', () => {
  const html = assignments.status({...base, github_status: {prs: {
    'https://github.com/o/r/pull/1': {error: 'Rate limited', evidence: {complete: true}},
    'https://github.com/o/r/pull/2': {evidence: {complete: false}}
  }}}, helpers);
  assert.match(html, /Rate limited/);
  assert.match(html, /Status unavailable/);
  assert.doesNotMatch(html, /All checks finished/);
});

test('review fetch errors keep the required failure visible', () => {
  const html = assignments.status({...base, github_status: {prs: {
    'https://github.com/o/r/pull/1': {error: 'Review access denied', evidence: {required: [{context: 'tests', state: 'failure'}], complete: false}}
  }}}, helpers);
  assert.match(html, /1 required check failed/);
  assert.match(html, /Review access denied/);
});
