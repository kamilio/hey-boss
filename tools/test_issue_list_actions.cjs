const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('src/issues/web/app.js', 'utf8');
const start = source.indexOf('function issueListActions(');
assert.ok(start >= 0, 'List actions must have a shared state-aware definition');
const context = vm.createContext({model:{boss:{name:'Boss'}}, own:id=>id==='human:boss'});
vm.runInContext(source.slice(start, source.indexOf('\nfunction ', start + 1)), context);
const actions = issue => context.issueListActions({state:'open',labels:[],...issue});
const ids = issue => Array.from(actions(issue), item=>item.action);
assert.deepEqual(ids({}), ['view','copy','assign_boss','draft','close','block']);
assert.deepEqual(ids({assignee:'human:boss'}), ['view','copy','unassign','close','block']);
assert.deepEqual(ids({assignee:'codex:other'}), ['view','copy','assign_boss','unassign','close','block']);
assert.deepEqual(ids({draft:true}), ['view','copy','undraft','close','block']);
assert.deepEqual(ids({state:'blocked'}), ['view','copy','reopen','draft','close']);
assert.deepEqual(ids({state:'blocked',draft:true}), ['view','copy','reopen','close']);
assert.equal(actions({state:'blocked'}).find(a=>a.action==='reopen').label, 'Unblock issue');
assert.ok(actions({state:'blocked',blocked_by:[{number:2}]}).find(a=>a.action==='reopen').disabled);
assert.deepEqual(ids({state:'closed'}), ['view','copy','reopen']);
assert.deepEqual(ids({deleted_at:1}), ['view','copy','restore']);
assert.equal(actions({state:'closed'}).find(a=>a.action==='reopen').label, 'Reopen issue');
assert.deepEqual(ids({state:'ready',assignee:'human:boss'}), ['view','copy','reopen','unassign','close']);
assert.deepEqual(ids({state:'ready',draft:true}), ['view','copy','reopen','close']);
assert.deepEqual(ids({state:'blocked',manual_blocked:true,blocked_by:[{number:2}]}), ['view','copy','clear_manual_hold','draft','close']);
assert.equal(actions({state:'blocked',manual_blocked:true}).find(a=>a.action==='clear_manual_hold').label, 'Release hold');
assert.deepEqual(ids({attempt_hold:{host:'test-machine'}}), ['view','copy']);
context.model.drafts_enabled=false;
assert.deepEqual(ids({}), ['view','copy','assign_boss','close','block']);
assert.ok(ids({draft:true}).includes('undraft'), 'Existing drafts can still be marked ready when drafts are disabled');
console.log('COMPLETE: 18/18 issue list action state checks');

const actionStart = source.indexOf('async function performAction(');
const calls = [], confirmations = [];
Object.assign(context, {
  model:{boss:{name:'Boss'},project:{id:'different-project'},route:{host:'different-host'},detail:null},
  actorName:id=>id, $:()=>({value:'Unrelated hidden comment'}),
  confirmDialog:async (...args)=>{confirmations.push(args);return true;},
  mutate:async (...args)=>{calls.push(args);}, toast(){},
  renderRoute:async()=>{throw Error('List action must not rerender a different route');},
  refreshProjects:async()=>{throw Error('List owner controls refresh');},
});
vm.runInContext(source.slice(actionStart, source.indexOf('async function submitComment(', actionStart)), context);
(async () => {
  const target={issue:{number:12,version:7,assignee:'codex:worker'},project:'original-project',host:'original-host',sequence:1};
  await context.performAction('unassign',{},target);
  assert.equal(confirmations.length,1,'Unassigning another agent requires confirmation');
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0])),[{action:'unassign',number:12,force:true},'original-project','original-host']);
  context.confirmDialog=async()=>false;
  await context.performAction('close',{},target);
  assert.equal(calls.length,1,'Cancelling must not mutate');
  context.confirmDialog=async()=>true;
  await context.performAction('reopen',{},target);
  assert.equal(calls[1][0].if_version,7,'Reopening checks captured issue version');
  await context.performAction('block',{}, {...target,issue:{...target.issue,assignee:null}});
  assert.equal(calls[2][0].comment,null,'List actions never use hidden detail comments');
  await context.performAction('clear_manual_hold',{},target);
  assert.deepEqual(JSON.parse(JSON.stringify(calls[3][0])),{action:'reopen',number:12,clear_manual_hold:true,if_version:7},'Releasing a hold keeps the captured version and dependency checks');
  context.mutate=async()=>{throw Error('Connection lost');};
  await assert.rejects(context.performAction('close',{},target),/Connection lost/,'List owner handles failures without refreshing or announcing success');
  console.log('COMPLETE: 6/6 list mutation ownership and scope checks');
})().catch(error=>{console.error(error);process.exitCode=1;});
