const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('src/issues/web/app.js', 'utf8');
const context = vm.createContext({
  esc: value => String(value ?? '').replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;'),
  icon: () => '', avatar: () => '', date: () => 'today', actorName: id => id || 'Unassigned',
  model: {project: {id: 'named:QA', name: 'QA'}},
  assigneeActions: () => '<button>Assign</button>', renderReadiness: () => '<div>Readiness</div>',
  renderTagSidebar: () => '<div>Tags</div>',
  IssueBlockers: {card: () => '<button>Add blocker</button>'},
  HeyBossOrigin: {card: () => '<section>Origin</section>'},
  agentLaunchCount: () => '<button>Agent launches</button>',
});
const start = source.indexOf('function renderIssueWork(');
assert.ok(start >= 0, 'Issue work has an explicit group');
vm.runInContext(source.slice(start, source.indexOf('function renderDetail(', start)), context);
const value = {issue: {number: 1, version: 3, assignee: null}};
const work = context.renderIssueWork(value);
assert.match(work, /aria-label="Work"/);
assert.match(work, /Assignee/);
assert.match(work, /Readiness/);
assert.match(work, /Tags/);
assert.match(work, /Add blocker/);
assert.doesNotMatch(work, /Origin|Revision|Delete issue/);
const details = context.renderIssueContext(value.issue);
assert.match(details, /<details class="issue-context">/);
assert.match(details, /<summary>Issue details<\/summary>/);
assert.match(details, /Origin/);
assert.match(details, /Revision 3/);
assert.match(details, /Agent launches/);
assert.doesNotMatch(details, /<details[^>]*\bopen\b/);
const closed = context.renderIssueContext({...value.issue, closed_by: 'human:boss'});
assert.match(closed, /Closed by human:boss/);
context.renderAllocation = () => '';
vm.runInContext(source.slice(source.indexOf('function draftUnavailable('), source.indexOf('function issueStateActions(')), context);
const readiness = issue => context.renderReadiness({issue, drafts_enabled: true});
assert.match(readiness({state: 'open'}), /data-draft-action="draft"/);
for (const issue of [{state: 'closed'}, {state: 'open', assignee: 'codex:assigned'}, {state: 'blocked'}, {state: 'open', draft: true}]) {
  assert.doesNotMatch(readiness(issue), /data-draft-action="draft"/);
}
assert.match(readiness({state: 'open', assignee: 'codex:assigned'}), /Assigned/);
assert.match(readiness({state: 'closed'}), /Completed/);
assert.equal(readiness({deleted_at: 1}), '');
console.log('Issue architecture groups preserve controls and history without default metadata clutter');
