const assert = require('node:assert/strict');
const test = require('node:test');
const vm = require('node:vm');
const context = vm.createContext({});
vm.runInContext(require('node:fs').readFileSync('src/issues/web/components.js', 'utf8') + '\nthis.ui = HeyBossUI;', context);
global.HeyBossUI = context.ui;
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
  assert.equal(describe({waiting: true}).detail, 'Waiting for new GitHub findings.');
  assert.match(describe({waiting: false}).detail, /queued/);
  assert.match(describe({actor: 'codex:123', machine_name: 'Devbox'}).detail, /Codex.*Devbox/);
  assert.equal(describe({waiting: true}).label, 'GitHub PR watcher');
  assert.equal(describe({waiting: false}).label, 'Unassigned');
  assert.equal(describe({actor: 'codex:123', machine_name: 'Devbox'}).label, 'Codex');
});

test('watcher assignment selector shows the current owner while retaining the watcher destination', () => {
  for (const [extra, selected, label] of [
    [{waiting:true}, 'github', 'GitHub PR watcher'],
    [{waiting:false}, 'active', 'Unassigned'],
    [{actor:'codex:123', machine_name:'Devbox'}, 'active', 'Codex'],
    [{waiting:false, machine:'box', machine_name:'Devbox'}, 'machine:box', 'Devbox'],
  ]) {
    const issue = {...base, assignment:{kind:'github', ...extra}};
    const html = assignments.render({issue, project:{id:'named:QA'}}, helpers);
    assert.match(html, new RegExp(`value="${selected}"[^>]* selected[^>]*>${label}</option>`));
    assert.match(html, /value="github"/);
    assert.match(html, /data-action="refresh_github"/);
    assert.match(html, /value="unassigned"[^>]*>Unassigned \(stop monitoring\)<\/option>/);
    if (extra.actor) assert.match(html, /agent=codex%3A123/);
  }
});

test('confirmed conflicts stay visible alongside CI and incomplete fetches', () => {
  const status = extra => assignments.status({...base, github_status:{prs:{
    'https://github.com/o/r/pull/1':{evidence:{conflicts:'conflicting', required:[], complete:false, ...extra}},
  }}}, helpers);
  assert.match(status({}), /class="github-check-summary failed">Merge conflicts/);
  assert.match(status({required:[{context:'tests',state:'failure'}]}), /Merge conflicts/);
  assert.match(status({policy_errors:[{message:'Policy unavailable'}]}), /Merge conflicts/);
  assert.doesNotMatch(status({sources_match:false}), />Merge conflicts/);
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
  for (const pr of [{url:'https://example.com/o/r/pull/1'}, {url:'https://github.com/o/r/pull/0'}, {url:'https://github.com/../r/pull/1'}, {url:'https://github.com/o/./pull/1'}, {url:'https://github.com/o/r/pull/18446744073709551616'}, {url:'https://github.com/o/r/pull/1',status:'closed'}]) {
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

test('review-only pull requests do not claim that CI ran', () => {
  const html = assignments.status({...base, github_status:{prs:{'https://github.com/o/r/pull/1':{evidence:{complete:true,ci_settled:true,ci_complete:false,has_checks:false,required_state:'not_required',reviews:[{body:'Please fix the race'}]}}}}},helpers);
  assert.match(html,/No checks reported/);
  assert.match(html,/Please fix the race/);
  assert.doesNotMatch(html,/All checks finished|Checks in progress/);
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

test('policy errors stay visible alongside last observed required failures', () => {
  const html = assignments.status({...base, github_status:{prs:{'https://github.com/o/r/pull/1':{evidence:{required:[{context:'tests',state:'failure'}],policy_errors:[{source:'branch_protection',message:'Policy access denied <retry>'}],complete:false}}}}},helpers);
  assert.match(html,/1 required check failed \(policy incomplete\)/);
  assert.match(html,/Policy access denied &lt;retry&gt;/);
  assert.doesNotMatch(html,/<retry>|All checks finished/);
});

test('mismatched source heads label old checks instead of attributing failure to the new head', () => {
  const html = assignments.status({...base, github_status:{prs:{'https://github.com/o/r/pull/1':{evidence:{sources_match:false,required:[{context:'tests',state:'failure'}],complete:false}}}}},helpers);
  assert.match(html,/Refreshing changed pull request/);
  assert.match(html,/Last recorded required checks/);
  assert.doesNotMatch(html,/1 required check failed|All checks finished|github-check-summary failed/);
});

test('stopped watchers explain missing PRs without advertising queued work', () => {
  const issue = {...base, state:'closed', assignment:{kind:'github'}, github_status:{monitoring:false, stopped_reason:'no_open_pull_requests', prs:{}}};
  assert.match(assignments.describe(issue, helpers).detail, /Monitoring stopped/);
  const html = assignments.status(issue, helpers);
  assert.match(html, /No open GitHub pull requests remain/);
  assert.doesNotMatch(html, /Waiting for the first GitHub status/);
});

test('condensed GitHub evidence identifies omitted results', () => {
  const html = assignments.status({...base, github_status:{prs:{'https://github.com/o/r/pull/1':{evidence:{truncated:true,omitted:{checks:80,reviews:7},required:[]}}}}},helpers);
  assert.match(html,/80 checks/);
  assert.match(html,/7 reviews/);
  assert.match(html,/GitHub/);
});

test('required failure count includes results omitted from the summary', () => {
  const html = assignments.status({...base, github_status:{prs:{'https://github.com/o/r/pull/1':{evidence:{truncated:true,omitted:{required:236},required_counts:{total:300,failure:200,satisfied:100},required:[{context:'one displayed check',state:'failure'}]}}}}},helpers);
  assert.match(html,/200 required checks failed/);
  assert.match(html,/236 required checks omitted/);
});

test('a bounded multi-PR status discloses omitted PRs and source errors', () => {
  const html = assignments.status({...base,github_status:{omitted_prs:6,prs:{'https://github.com/o/r/pull/1':{evidence:{complete:false,truncated:true,omitted:{source_errors:2},required:[]}}}}},helpers);
  assert.match(html,/6 additional pull requests/);
  assert.match(html,/Status incomplete/);
  assert.match(html,/2 source errors/);
});

test('a surviving attempt disables reassignment and never promises pickup', () => {
  const issue={...base,attempt_hold:{attempt_id:'retained'},assignment:{kind:'unassigned'}};
  const html=assignments.render({issue},helpers);
  assert.match(html,/data-assignment-select[^>]*disabled/);
  assert.match(html,/Pickup is paused until the retained attempt is reconciled/);
  assert.doesNotMatch(html,/An available machine can pick this up/);
});

test('assignment shows actual fetch activity and a fetch-now action', () => {
  const url=base.pull_requests[0].url;
  const html=assignments.render({issue:{...base,assignment:{kind:'github',waiting:true},github_status:{monitoring:true,fetches:{[url]:{finished_at:Date.now()-60000,next_at:Date.now()+30000}},prs:{[url]:{evidence:{required_state:'pending'}}}}}},helpers);
  assert.match(html,/Last fetch/);
  assert.match(html,/<time datetime=/);
  assert.match(html,/data-action="refresh_github"/);
  assert.match(html,/Fetch now/);
});

test('queued fetches and stopped monitoring are clear in assignment', () => {
  const url=base.pull_requests[0].url;
  const issue={...base,assignment:{kind:'github',waiting:true},github_status:{monitoring:true,fetches:{[url]:{requested_at:Date.now()}},prs:{}}};
  assert.match(assignments.render({issue},helpers),/Fetch queued/);
  const stopped=assignments.render({issue:{...issue,state:'closed',github_status:{...issue.github_status,monitoring:false}}},helpers);
  assert.doesNotMatch(stopped,/data-action="refresh_github"/);
});

test('fetch errors are escaped and interrupted attempts allow another fetch', () => {
  const url=base.pull_requests[0].url, now=Date.now();
  const issue={...base,assignment:{kind:'github',actor:'codex:123',machine_name:'Devbox'},github_status:{monitoring:true,fetches:{[url]:{started_at:now-180000,finished_at:now-240000,error:'retry <unsafe>'}},prs:{}}};
  const html=assignments.render({issue},helpers);
  assert.match(html,/Codex is working on Devbox/);
  assert.match(html,/Fetch interrupted/);
  assert.match(html,/retry &lt;unsafe&gt;/);
  assert.doesNotMatch(html,/refresh_github" disabled|<unsafe>/);
});

test('multiple PRs show their own last fetch and retry time', () => {
  const now=Date.now(), url=base.pull_requests[0].url, other='https://github.com/o/r/pull/2';
  const issue={...base,pull_requests:[...base.pull_requests,{url:other}],assignment:{kind:'github'},github_status:{fetches:{[url]:{finished_at:now-10000},[other]:{finished_at:now-200000,next_at:now+300000,error:'Rate limited'}}}};
  const html=assignments.render({issue},helpers);
  assert.match(html,/PR #1/);
  assert.match(html,/PR #2/);
  assert.equal((html.match(/Last fetch/g)||[]).length,2);
  assert.match(html,/Retry after/);
  assert.match(html,/Rate limited/);
});

test('waiting watcher keeps help in a popover and avoids duplicate empty status', () => {
  const issue = {...base, assignment:{kind:'github', waiting:true}};
  const html = assignments.render({issue}, helpers);
  assert.match(html, /popovertarget="assignment-help"/);
  assert.match(html, /id="assignment-help"[^>]*popover/);
  assert.doesNotMatch(html, /<p[^>]*>Waiting for required/);
  assert.doesNotMatch(html, /Not recorded yet|Last fetch/);
  assert.match(html, /Awaiting first fetch/);
  assert.equal(assignments.status(issue, helpers), '');
});
