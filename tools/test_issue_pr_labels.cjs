const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync('src/issues/web/app.js', 'utf8');
const context = vm.createContext({
  URL,
  esc: value => String(value).replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;'),
  icon: () => '',
  traceLinkForOrigin: () => '',
});
vm.runInContext(
  source.slice(source.indexOf('function prStatus('), source.indexOf('function renderList(')) +
  source.slice(source.indexOf('const PR_PURPOSES ='), source.indexOf('function prPurposeOptions(')),
  context,
);
const render = purpose => context.listPullRequests({pull_requests: [{url: 'https://github.com/poe-internal/poe2/pull/15015', purpose}]});
for (const purpose of ['unspecified', undefined, null, '', 'unknown']) {
  const html = render(purpose);
  assert.ok(html.includes('<span class="pr-link-title">#15015</span>'));
  assert.ok(!html.includes('Unspecified'), `No unspecified text for ${purpose}`);
  assert.ok(!html.includes('pr-purpose-label'), `No empty badge for ${purpose}`);
  assert.ok(html.includes('aria-label="Open pull request #15015"'));
  assert.ok(html.includes('title="https://github.com/poe-internal/poe2/pull/15015"'));
}

vm.runInContext(
  source.slice(source.indexOf('function prPurposeOptions('), source.indexOf('async function changeCommit(')) +
  source.slice(source.indexOf('function renderPullRequests('), source.indexOf('async function changePullRequest(')),
  context,
);
const linkedIssue = {number: 1, pull_requests: [{url: 'https://github.com/example/repo/pull/123', purpose: 'fix'}]};
for (const issue of [{number: 1}, linkedIssue, {...linkedIssue, deleted_at: 1}]) {
  assert.equal(context.renderPullRequests(issue, false), '', 'Disabled PR workflow omits the whole sidebar section, including existing links');
  assert.match(context.renderCommits(issue, false), /Commits/, 'Direct-push workflow keeps its commit section');
}
assert.match(context.renderPullRequests({number: 1}, true), /No pull requests attached/, 'Enabled empty state remains available');
assert.match(context.renderPullRequests(linkedIssue, true), /id="pr-form"/, 'Enabled workflow can attach PRs');
assert.match(context.renderPullRequests(linkedIssue, true), /data-pr-purpose=/, 'Enabled workflow can classify existing PRs');
const deleted = context.renderPullRequests({...linkedIssue, deleted_at: 1}, true);
assert.match(deleted, /https:\/\/github.com\/example\/repo\/pull\/123/, 'Deleted issue retains PR history when enabled');
assert.doesNotMatch(deleted, /id="pr-form"|data-pr-purpose=|data-remove-pr=/, 'Deleted issue has no PR mutation controls');
console.log('Issue detail PR workflow checks passed');
for (const [purpose, label] of [['fix', 'Fix']]) {
  const html = render(purpose);
  assert.ok(html.includes(`<span class="pr-purpose-label">${label}</span>`));
  assert.ok(html.includes(`aria-label="Open pull request #15015 · ${label}"`));
}
for (const [purpose, label] of [['prerequisite', 'Prerequisite'], ['supporting-evidence', 'Supporting evidence']]) {
  assert.equal(render(purpose), '', `${label} links are omitted from the issue list`);
  assert.equal(context.prPurposeLabel(purpose), label, 'Details retain the purpose label');
}
assert.equal(context.prPurposeLabel('unspecified'), 'Unspecified', 'Details retain the purpose label');
assert.equal(context.listPullRequests({}), '');
console.log('Issue list PR labels passed');
for (const status of ['open', 'closed', 'merged', 'unknown']) {
  const html = context.listPullRequests({pull_requests:[{url:'https://github.com/o/r/pull/1',status,checked_at:Date.now()}]});
  assert.ok(html.includes(`pr-status-${status}`), `Show a tiny ${status} icon`);
}
const stale = context.listPullRequests({pull_requests:[{url:'https://github.com/o/r/pull/1',status:'open',error:'GitHub unavailable',checked_at:1}]});
assert.ok(stale.includes('pr-status-stale'));
for (const [status, age, delayed] of [
  ['open', 4 * 60_000, false],
  ['open', 8 * 60_000, true],
  ['closed', 20 * 60_000, false],
  ['closed', 33 * 60_000, true],
  ['merged', 24 * 60 * 60_000, false],
]) {
  const html = context.prStatus({status, checked_at: Date.now() - age});
  assert.equal(html.includes('pr-status-stale'), delayed, `${status} at ${age}ms respects its monitoring cadence`);
}
