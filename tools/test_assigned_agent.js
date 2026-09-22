const assert = require('node:assert/strict');
const {assignedAgentEntry, resolveAssignedAgent} = require('../src/issues/web/fleet.js');
const run = {id:'old',project_id:'named:QA',number:68,actor_id:'worker:old',session_id:'assigned-session',started_at:1,finished_at:2};
const data = {machines:[
  {host:'local',workers:[{runs:[{...run,id:'wrong-owner',actor_id:'worker:other',session_id:'other',started_at:100}]}]},
  {host:'devbox',state:'disconnected',workers:[{runs:[run,{...run,id:'latest',started_at:3,finished_at:null},
    {...run,id:'wrong-issue',number:69,started_at:4}, {...run,id:'wrong-project',project_id:'named:Other',started_at:5}]}]},
]};
const query = new URLSearchParams({project:'named:QA',issue:'68',agent:'codex:assigned-session'});
assert.equal(assignedAgentEntry(data,query).run.id,'latest','Select newest matching session, never another owner, issue or project');
assert.equal(assignedAgentEntry(data,query).machine.host,'devbox','Find the owning device even when disconnected');
query.set('agent','worker:old');
assert.equal(assignedAgentEntry(data,query).run.id,'latest','Match recorded actor IDs as well as claimed Codex sessions');
for(const [key,value] of [['agent','human:boss'],['agent','codex:unknown'],['project','named:Missing'],['issue','0']]){
  const other = new URLSearchParams(query); other.set(key,value);
  assert.equal(assignedAgentEntry(data,other),undefined,'Unavailable assignments must not select an unrelated trace');
}
assert.equal(assignedAgentEntry({},query),undefined);
(async()=>{
  const assignment=new URLSearchParams({project:'named:QA',issue:'68',agent:'codex:assigned-session'});
  const recent=await resolveAssignedAgent(data,assignment,()=>{throw Error('Recent sessions need no extra request');});
  assert.equal(recent.run.id,'latest');
  const saved={machine:{host:'local'},run:{id:'session:assigned-session',project_id:'named:QA',number:68,standalone:true}};
  const resolved=await resolveAssignedAgent({},assignment,async path=>{
    const url=new URL(path,'http://localhost');
    assert.equal(url.pathname,'/api/fleet/assignment');
    assert.equal(url.searchParams.get('project'),'named:QA');
    assert.equal(url.searchParams.get('issue'),'68');
    assert.equal(url.searchParams.get('agent'),'codex:assigned-session');
    return saved;
  });
  assert.equal(resolved,saved,'Load exact persisted assignment when recent activity is empty');
  await assert.rejects(resolveAssignedAgent({},assignment,async()=>{throw Error('Assignment changed');}),/Assignment changed/);
  console.log('Assigned agent trace checks passed');
})().catch(e=>{console.error(e);process.exitCode=1;});
