const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const elements = new Map();
const context = vm.createContext({
  model: {route: {}},
  HeyBossUI: {projectNavigation() {}},
  routeHash: route => new URLSearchParams(
    Object.entries(route).filter(([, value]) => value != null && value !== ''),
  ).toString(),
  $: selector => {
    if (!elements.has(selector)) elements.set(selector, {
      classList: {toggle() {}}, setAttribute() {}, removeAttribute() {},
    });
    return elements.get(selector);
  },
});
vm.runInContext(fs.readFileSync(require.resolve('../src/issues/web/inbox.js'), 'utf8'), context);

for (const view of ['issues', 'inbox']) {
  context.model.route = {
    view, project: 'named:Design & QA', host: 'devbox', issue: 41,
    notice: 'notice-123', state: 'closed', owner: 'human:boss',
    label: 'ready', search: 'navigation',
  };
  context.updateAppNavigation();
  const route = new URLSearchParams(elements.get('#nav-issues').href);
  assert.equal(route.has('issue'), false, `${view}: Issues must target the list`);
  assert.equal(route.has('notice'), false, `${view}: Issues must clear notice selection`);
  assert.equal(route.get('view'), 'issues');
  for (const key of ['project', 'host', 'state', 'owner', 'label', 'search'])
    assert.equal(route.get(key), context.model.route[key], `${view}: preserve ${key}`);
  assert.equal(context.model.route.issue, 41, 'Rendering links must not mutate the active route');
  assert.equal(new URLSearchParams(elements.get('#nav-inbox').href).has('issue'), false);
}
console.log('Issues navigation checks passed');
