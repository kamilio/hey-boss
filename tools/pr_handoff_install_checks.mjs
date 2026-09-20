// Verify an installed CLI in private disposable state; no real agents or issues.
import assert from 'node:assert/strict';
import {mkdtempSync,writeFileSync,readFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
import {setTimeout as delay} from 'node:timers/promises';

const binary=resolve(process.argv[2]), expected=process.argv[3];
const root=mkdtempSync(join(tmpdir(),'hey-boss-pr-handoff-check-'));
const env={...process.env,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:join(root,'fleet'),HEY_BOSS_INBOX_SOCKET:join(root,'no-inbox.sock'),HEY_BOSS_CODEX:join(root,'codex-fixture'),HEY_BOSS_HANDOFF_TEST_BINARY:binary};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR','HEY_BOSS_AGENT_ID'])delete env[key];
const run=args=>{
  const r=spawnSync(binary,args,{env,cwd:root,encoding:'utf8',timeout:30000});
  assert.equal(r.status,0,r.stderr||r.stdout||String(r.error));return r.stdout;
};
const cli=args=>JSON.parse(run(['issue','--project','PR handoff verification','--agent','human:verification','--json',...args]));
let child;
async function stop(){
  if(!child||child.exitCode!==null)return;
  const current=child;
  const ended=new Promise(r=>current.once('exit',r));current.kill('SIGTERM');
  const timer=setTimeout(()=>current.kill('SIGKILL'),5000);
  await ended;clearTimeout(timer);child=null;
}
async function worker(prs,number){
  child=spawn(binary,['worker','--project','PR handoff verification','--directory',root,prs?'--prs':'--no-prs'],{env,cwd:root,stdio:'ignore'});
  const deadline=Date.now()+30000;
  let status;
  do{
    await delay(100);status=cli(['worker','status']);
    if(status.runs?.some(r=>r.number===number&&r.finished_at))break;
    assert(Date.now()<deadline,'Worker verification timed out');
  }while(true);
  assert.equal(status.runs.find(r=>r.number===number).state,'completed');
  assert.equal(status.eligible,0);await stop();
}
try{
  const version=run(['--version']).trim();if(expected)assert(version.includes(expected),'Installed build mismatch');
  writeFileSync(env.HEY_BOSS_CODEX,`#!/usr/bin/env node
const {createInterface}=require('node:readline');
const {spawnSync}=require('node:child_process');
const {randomUUID}=require('node:crypto');
const {writeFileSync,existsSync}=require('node:fs');
const session=randomUUID(),send=v=>process.stdout.write(JSON.stringify(v)+'\\n');
const cli=args=>{const r=spawnSync(process.env.HEY_BOSS_HANDOFF_TEST_BINARY,['issue','--json','--agent','codex:'+session,...args],{encoding:'utf8'});if(r.status!==0)throw Error(r.stderr||r.stdout);};
createInterface({input:process.stdin}).on('line',line=>{
 const m=JSON.parse(line);if(!m.method||m.id===undefined)return;
 let result={};
 if(m.method==='thread/start')result={thread:{id:session}};
 if(m.method==='turn/start'){
   const text=m.params.input[0].text,number=text.match(/issue view (\\d+)/)[1];
   cli(['claim',number]);writeFileSync('prompt.txt',text);
   if(existsSync('handoff'))cli(['assign-to-boss',number]);
   send({id:m.id,result:{turn:{id:'fixture-turn'}}});
   send({method:'item/completed',params:{threadId:session,item:{type:'agentMessage',text:JSON.stringify({status:'completed',summary:'Fix ready; CI and reviews complete. Merge remains.'})}}});
   send({method:'turn/completed',params:{threadId:session,turn:{id:'fixture-turn',status:'completed'}}});return;
 }
 send({id:m.id,result});
});
`,{mode:0o700});
  cli(['create','--title','Open fix PR']);
  cli(['comment','1','--body','Existing history']);
  cli(['pr','add','1','https://github.com/example/repo/pull/1','--purpose','fix']);
  cli(['pr','add','1','https://github.com/example/repo/pull/2','--purpose','supporting-evidence']);
  await worker(true,1);
  const view=cli(['view','1']);
  assert.equal(view.issue.state,'open');assert.equal(view.issue.assignee,'human:boss');assert.equal(view.issue.closed_at,null);
  assert.equal(view.comments.length,2);
  assert.deepEqual(cli(['pr','list','1']).pull_requests.map(p=>p.purpose),['fix','supporting-evidence']);
  assert(readFileSync(join(root,'prompt.txt'),'utf8').includes('hey-boss issue assign-to-boss 1'));
  cli(['create','--title','Direct main completion']);await worker(false,2);
  assert.equal(cli(['view','2']).issue.state,'closed');
  cli(['create','--title','Agent explicitly hands to Boss']);writeFileSync(join(root,'handoff'),'');
  await worker(true,3);
  assert.equal(cli(['view','3']).issue.state,'open');assert.equal(cli(['view','3']).issue.assignee,'human:boss');
  child=spawn(binary,['issue','web','--project','PR handoff verification','--port','0','--no-discovery','--json'],{env,cwd:root,stdio:['ignore','pipe','ignore']});
  const info=await new Promise((yes,no)=>{
    let line='';const timer=setTimeout(()=>no(Error('Web verification timed out')),20000);
    child.stdout.on('data',chunk=>{line+=chunk;if(line.includes('\n')){clearTimeout(timer);yes(JSON.parse(line.split('\n')[0]));}});
    child.on('error',no);child.on('exit',()=>{clearTimeout(timer);no(Error('Web verification exited'));});
  });
  const html=await(await fetch(info.url)).text();assert(html.includes('id="project-pr-handoff-help"'));
  await stop();
  console.log(JSON.stringify({status:'passed',version,open_boss_handoff:true,pickup_excluded:true,links_and_history_preserved:true,explicit_agent_handoff:true,non_pr_closure:true,installed_ui:true}));
}finally{await stop();rmSync(root,{recursive:true,force:true});}
