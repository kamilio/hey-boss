// Exercise the public CLI against a disposable database, never real assignments.
import assert from 'node:assert/strict';
import {mkdtempSync, rmSync, mkdirSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
const binary=resolve(process.env.HEY_BOSS_TEST_BINARY||'target/debug/hey-boss');
const root=mkdtempSync(join(tmpdir(),'boss-unassign-'));
const env={...process.env,HOME:root,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:join(root,'fleet')};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID','HEY_BOSS_FLEET_CONFIG','HEY_BOSS_FLEET_DESIRED','HEY_BOSS_FLEET_SUPERVISED'])delete env[key];
mkdirSync(env.HEY_BOSS_FLEET_STATE);
const issue=(actor,args,ok=true)=>{
  const r=spawnSync(binary,['issue','--project','named:Unassign fixture','--agent',actor,'--json',...args],{cwd:root,env,encoding:'utf8',timeout:30000});
  assert.equal(r.signal,null,r.stderr);const value=JSON.parse(r.stdout);
  assert.equal(r.status===0,ok,JSON.stringify(value));return value;
};
try{
  const initialized=spawnSync(binary,['project','init','--project','named:Unassign fixture','--prs','false','--worktree','false','--yes','--json'],{cwd:root,env,encoding:'utf8',timeout:30000});
  assert.equal(initialized.status,0,initialized.stderr||initialized.stdout);
  for(const command of [['unassign','1'],['assign','1','unassigned'],['unassign','1','--supervisor']]){
    if(command[0]==='unassign'&&command.length===2)issue('codex:owner',['create','--title','Boss-owned fixture','--body','Preserve this body']);
    issue('codex:owner',['assign-to-boss','1']);
    const result=issue('codex:delegate',[...command,'--request-id',command.join('-')]);
    assert.equal(result.issue.assignee,null);
    assert.equal(result.issue.state,'open');
    assert.equal(result.issue.body,'Preserve this body');
    const replay=issue('codex:delegate',[...command,'--request-id',command.join('-')]);
    assert.equal(replay.issue.version,result.issue.version);
  }
  issue('codex:owner',['claim','1']);
  const before=issue('codex:owner',['view','1']);
  assert.equal(issue('codex:delegate',['unassign','1'],false).error.code,'conflict');
  const after=issue('codex:owner',['view','1']);
  assert.equal(after.issue.assignee,'codex:owner');assert.equal(after.issue.version,before.issue.version);
  issue('codex:owner',['unassign','1']);
  assert.equal(issue('codex:owner',['view','1']).issue.assignee,null);
  console.log('Boss unassign CLI: ordinary command, assignment alias, supervisor route, retries, and other-owner protection passed');
}finally{rmSync(root,{recursive:true,force:true});}
