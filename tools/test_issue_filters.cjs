const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const context = vm.createContext({URLSearchParams});
for (const file of ['blockers', 'filters']) vm.runInContext(fs.readFileSync(`src/issues/web/${file}.js`, 'utf8'), context);
const filters = vm.runInContext('IssueFilters', context);

test('launch thresholds normalize safely and blocked reasons stay in Blocked', () => {
  for (const value of ['', '0', '-1', '1.5', '5x', '05', '9007199254740992'])
    assert.equal(filters.read(new URLSearchParams({min_launches:value})).min_launches, '');
  assert.equal(filters.read(new URLSearchParams('min_launches=5')).min_launches, '5');
  assert.equal(filters.read(new URLSearchParams('state=blocked&blocked=hold')).blocked, 'hold');
  assert.equal(filters.read(new URLSearchParams('state=open&blocked=hold')).blocked, '');
});

test('launch and blocked refinements intersect before pagination', () => {
  const issues = [
    {number:1, state:'blocked', manual_blocked:true, agent_launch_count:4},
    {number:2, state:'blocked', manual_blocked:true, agent_launch_count:5},
    {number:3, state:'blocked', blocked_by:[{number:9}], agent_launch_count:8},
    {number:4, state:'blocked', manual_blocked:true, blocked_by:[{number:9}], agent_launch_count:10},
    {number:5, state:'blocked', attempt_hold:{active:true}, agent_launch_count:5},
  ];
  assert.deepEqual(issues.filter(i => filters.matches(i, {min_launches:'5', blocked:'hold'})).map(i => i.number), [2,4,5]);
  assert.deepEqual(issues.filter(i => filters.matches(i, {min_launches:'5', blocked:'dependencies'})).map(i => i.number), [3,4]);
  assert.equal(filters.matches({state:'open'}, {min_launches:'5'}), false);
  assert.equal(filters.matches({...issues[1], deleted_at:123}, {blocked:'hold'}), false);
});

test('presets appear in every section and toggle independently', () => {
  for (const state of ['open', 'blocked', 'ready', 'closed', 'deleted', 'all']) {
    const route = {state, owner:'all', min_launches:'', search:'keep', label:'bug'};
    const presets = filters.presets(route);
    assert.equal(presets.length, state === 'blocked' ? 3 : 2);
    const launches = presets.find(p => p.key === 'min_launches');
    const boss = presets.find(p => p.key === 'owner');
    Object.assign(route, filters.toggle(route, launches), filters.toggle(route, boss));
    assert.equal(route.min_launches, '5');
    assert.equal(route.owner, 'human:boss');
    Object.assign(route, filters.toggle(route, launches));
    assert.equal(route.min_launches, '');
    assert.equal(route.owner, 'human:boss');
    assert.equal(route.search, 'keep');
    assert.equal(route.label, 'bug');
    assert.equal(filters.toggle({...route, owner:'mine'}, boss).owner, 'all');
  }
});
