const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const context = vm.createContext({});
vm.runInContext(fs.readFileSync('src/issues/web/blockers.js', 'utf8') + '\nthis.blockers = IssueBlockers;', context);
const {kind, matches} = context.blockers;
const hold = {state:'blocked', manual_blocked:true};
const dependency = {state:'blocked', manual_blocked:false, blocked_by:[{number:1}]};
const mixed = {...dependency, manual_blocked:true};
assert.equal(kind(hold), 'hold');
assert.equal(kind(dependency), 'dependencies');
assert.equal(kind(mixed), 'hold', 'A hold still needs attention when dependencies also exist');
assert.equal(kind({state:'open',manual_blocked:true}), '');
assert.equal(kind({...hold,deleted_at:1}), '');
assert.ok(matches(mixed, 'hold'));
assert.ok(matches(mixed, 'dependencies'), 'Dependency filter includes all issues with unfinished dependencies');
assert.ok(!matches(hold, 'dependencies'));
assert.ok(!matches(dependency, 'hold'));
assert.ok(matches(hold, ''));
console.log('COMPLETE: blocked issue classification and overlapping filters');

const app = fs.readFileSync('src/issues/web/app.js', 'utf8');
context.icon = () => '';
context.model = {};
vm.runInContext(app.slice(app.indexOf('function issueStateActions('), app.indexOf('function renderDraftNotice(')), context);
const closedActions = context.issueStateActions({...dependency, state:'closed'});
assert.match(closedActions, /data-action="reopen"/);
assert.doesNotMatch(closedActions, /disabled/, 'Closed issues can reopen into dependency waiting');
assert.match(context.issueStateActions(dependency), /disabled/, 'Reopen cannot bypass active dependencies');
console.log('COMPLETE: closed issue reopening retains dependency protection');

vm.runInContext(fs.readFileSync('src/issues/web/components.js', 'utf8'), context);
context.esc = s => String(s ?? "");
context.routeHash = () => "#test";
const cardHtml = context.blockers.card({
  number: 2,
  title: "API layer",
  state: "open",
  blocked_by: [],
  blocker_links: [{
    number: 1,
    title: "Storage layer",
    state: "ready",
    satisfied: true,
    pull_requests: [{url: "https://github.com/example/repo/pull/101"}]
  }],
  blocking: [{
    number: 3,
    title: "Web UI",
    state: "blocked",
    actively_blocked: true,
    unblocks_on_release: true,
    pull_requests: []
  }]
});
assert.match(cardHtml, /dep-chain-graph/);
assert.match(cardHtml, /PR #101/);
assert.match(cardHtml, /Unblocks on Ready\/Close/);
assert.match(cardHtml, /data-create-dependent="2"/);
console.log("COMPLETE: dependency chain and PR stack visualization");
