const {test} = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const app = fs.readFileSync('src/issues/web/app.js','utf8');
const context = vm.createContext({URLSearchParams});
vm.runInContext(fs.readFileSync('src/issues/web/components.js','utf8') + '\nthis.HeyBossUI = HeyBossUI;', context);
vm.runInContext(fs.readFileSync('src/issues/web/assignments.js','utf8'), context);
vm.runInContext(`const model = {project:{id:'named:QA'},boss:{name:'Boss'},route:{host:'devbox',state:'open',search:'keep',label:'bug',owner:'all'}};
const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const icon = HeyBossUI.icon;
const actorName = id => HeyBossUI.actorLabel(id, 'gpt-6-astra');
const routeHash = route => '#' + new URLSearchParams(Object.entries(route).filter(([,v]) => v != null));
const HeyBossOrigin = {conversation: () => null};
${app.slice(app.indexOf('function traceLinkForOrigin('),app.indexOf('function renderLabelFilter(')) .split('let assignmentCardContext')[0]}
this.render = listAssignment;`, context);
for (const [kind,owner,symbol] of [['boss','human:boss','hat'],['machine','machine:box','monitor'],['agent','codex:session','codex'],['github','watcher:github','pull-request']]) {
  test(`${kind} badge filters exact owner and moves details into a card`, () => {
    const html = context.render({number:7,assignment:{kind,machine:'box',machine_name:'Devbox',waiting:kind==='github',actor:kind==='agent'?'codex:session':null}});
    assert.match(html,/class="[^"]*assignment-badge/);
    assert.match(html,new RegExp(`data-assignment-icon="${symbol}"`));
    assert.match(html,/popover="manual"/);
    assert.match(html,new RegExp('owner='+encodeURIComponent(owner)));
    assert.match(html,/host=devbox/);
    assert.match(html,/search=keep/);
    assert.match(html,/Filter by/);
    if(kind==='agent') { assert.match(html,/Codex · gpt-6-astra/); assert.match(html,/Open agent/); }
    if(kind==='github') assert.match(html,/data-open-watcher="7"/);
  });
}
test('watcher card includes its active agent conversation', () => {
  const html=context.render({number:7,assignment:{kind:'github',actor:'codex:session'}});
  assert.match(html,/data-assignment-kind="agent"/);
  assert.match(html,/data-assignment-icon="codex"/);
  assert.match(html,/owner=codex%3Asession/);
  assert.doesNotMatch(html,/Filter by GitHub PR watcher/);
  assert.match(html,/agent=codex%3Asession/);
  assert.match(html,/Open agent/);
});

test('released watcher shows unassigned ownership and keeps status accessible', () => {
  const html=context.render({number:7,assignee:null,assignment:{kind:'github',waiting:false}});
  assert.match(html,/data-assignment-kind="unassigned"/);
  assert.match(html,/owner=unassigned/);
  assert.match(html,/>Unassigned<\/strong>/);
  assert.match(html,/data-open-watcher="7"/);
  assert.doesNotMatch(html,/Open agent conversation|Filter by GitHub PR watcher/);
});
test('names are escaped and historical traces remain accessible', () => {
  assert.doesNotMatch(context.render({number:1,assignment:{kind:'machine',machine:'box',machine_name:'<img src=x>'}}),/<img/);
  assert.equal(context.render({number:2,assignment:{kind:'unassigned'}}),'');
  const html=context.render({number:3,assignment:{kind:'unassigned'},closed_by:'codex:old'});
  assert.match(html,/Open agent/);
  assert.doesNotMatch(html,/Filter by/);
});

test('scheduled service ownership never invents an agent session', () => {
  const issue={number:9,job_run_id:'run',assignment:{kind:'agent',actor:'job:run'}};
  const html=context.render(issue);
  assert.match(html,/Scheduled job/);
  assert.match(html,/dedicated job service/);
  assert.doesNotMatch(html,/Agent is working|Open agent conversation|agents\/session/);
  const detail=context.IssueAssignments.render({issue,project:{id:'named:QA'}},{actorName:id=>id,bossName:'Boss'});
  assert.doesNotMatch(detail,/Agent conversation|agents\/session/);
  assert.equal(context.render({...issue,assignment:{kind:'unassigned'},closed_by:'job:run'}),'');
  assert.doesNotMatch(context.IssueAssignments.describe({...issue,assignment:{kind:'unassigned'}},{actorName:id=>id,bossName:'Boss'}).detail,/pick this up/);
  const running=context.render({...issue,assignment:{kind:'agent',actor:'codex:exact-session'}});
  assert.match(running,/agent=codex%3Aexact-session/);
});
