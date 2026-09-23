const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync('src/issues/web/app.js', 'utf8');
const context = vm.createContext({
  URL,
  esc: value => String(value).replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;'),
  icon: () => '',
});
vm.runInContext(
  source.slice(source.indexOf('function listPullRequests('), source.indexOf('function renderList(')) +
  source.slice(source.indexOf('const PR_PURPOSES ='), source.indexOf('function prPurposeOptions(')),
  context,
);
const render = purpose => context.listPullRequests({pull_requests: [{url: 'https://github.com/poe-internal/poe2/pull/15015', purpose}]});
for (const purpose of ['unspecified', undefined, null, '', 'unknown']) {
  const html = render(purpose);
  assert.ok(html.includes('poe-internal/poe2#15015'));
  assert.ok(!html.includes('Unspecified'), `No unspecified text for ${purpose}`);
  assert.ok(!html.includes('pr-purpose-label'), `No empty badge for ${purpose}`);
  assert.ok(html.includes('aria-label="Open pull request poe-internal/poe2#15015"'));
  assert.ok(html.includes('title="https://github.com/poe-internal/poe2/pull/15015"'));
}
for (const [purpose, label] of [['fix', 'Fix']]) {
  const html = render(purpose);
  assert.ok(html.includes(`<span class="pr-purpose-label">${label}</span>`));
  assert.ok(html.includes(`aria-label="Open pull request poe-internal/poe2#15015 · ${label}"`));
}
for (const [purpose, label] of [['prerequisite', 'Prerequisite'], ['supporting-evidence', 'Supporting evidence']]) {
  assert.equal(render(purpose), '', `${label} links are omitted from the issue list`);
  assert.equal(context.prPurposeLabel(purpose), label, 'Details retain the purpose label');
}
assert.equal(context.prPurposeLabel('unspecified'), 'Unspecified', 'Details retain the purpose label');
assert.equal(context.listPullRequests({}), '');
console.log('Issue list PR labels passed');
