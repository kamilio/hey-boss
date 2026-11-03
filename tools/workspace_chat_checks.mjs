// Run with node tools/workspace_chat_checks.mjs /absolute/path/to/hey-boss.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {chmod, mkdtemp, readFile, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {createInterface} from 'node:readline';
const dir = await mkdtemp(join(tmpdir(), 'workspace-chat-'));
const fake = join(dir, 'codex');
try {
  // Exercise the real bridge, approval round trip, cwd and second turn on one thread.
  await writeFile(fake, `#!/usr/bin/env node
const rl=require('node:readline').createInterface({input:process.stdin}), fs=require('node:fs');
const send=v=>console.log(JSON.stringify(v)); let n=0;
rl.on('line', line=>{
 const q=JSON.parse(line), threadId='chat-fixture', turnId='turn-'+n;
 if(q.method==='thread/start')send({id:q.id,result:{thread:{id:threadId}}});
 else if(q.method==='turn/start'){
   n++; const turnId='turn-'+n;
   send({id:q.id,result:{turn:{id:turnId}}});
   send({method:'item/started',params:{threadId,turnId,item:{id:'cmd',type:'commandExecution',command:'pwd'}}});
   send({id:'ask',method:'item/commandExecution/requestApproval',params:{threadId,turnId,command:'pwd'}});
 } else if(q.id==='ask'){
   fs.appendFileSync('decisions',q.result.decision+'\\n');
   send({method:'item/completed',params:{threadId,turnId,item:{id:'cmd',type:'commandExecution',status:q.result.decision==='accept'?'completed':'declined'}}});
   send({method:'item/agentMessage/delta',params:{threadId,turnId,delta:'reply '+n}});
   send({method:'item/completed',params:{threadId,turnId,item:{type:'agentMessage',text:'reply '+n}}});
   send({method:'turn/completed',params:{threadId,turn:{id:turnId,status:'completed'}}});
 } else if(q.id!==undefined)send({id:q.id,result:{}});
});
`);
  await chmod(fake, 0o755);
  const child=spawn(resolve(process.argv[2]), ['workspace-chat'], {cwd:dir, env:{...process.env, HEY_BOSS_CODEX:fake}, stdio:['pipe','pipe','inherit']});
  const exit=new Promise(res=>child.on('close',res));
  const timeout=setTimeout(()=>child.kill(), 30000);
  const commands=o=>child.stdin.write(JSON.stringify(o)+'\n');
  let turns=0, starts=0, ends=0, messages=0, streamed=0;
  commands({turn:'Hello'});
  for await(const line of createInterface({input:child.stdout})){
    const e=JSON.parse(line);
    if(e.ToolStarted){assert.equal(e.ToolStarted.input.command,'pwd'); starts++;}
    if(e.Approval)commands({decision:e.Approval.id,allow:turns===1});
    if(e.ToolCompleted){assert.equal(e.ToolCompleted.failed,turns===0); ends++;}
    if(e.TextDelta){assert.equal(e.TextDelta.text,'reply '+(turns+1)); streamed++;}
    if(e.Message){assert.equal(e.Message.text,'reply '+(turns+1)); messages++;}
    if(e.TurnCompleted){assert.equal(e.TurnCompleted.output,'reply '+(++turns)); if(turns===1)commands({turn:'Hello'}); else child.stdin.end();}
  }
  clearTimeout(timeout);
  assert.equal(await exit,0); assert.deepEqual([turns,starts,ends,streamed,messages],[2,2,2,2,2]); assert.equal(await readFile(join(dir,'decisions'),'utf8'),'decline\naccept\n');
  console.log('PASS workspace cwd, tool events, decline and allow, second turn, disconnect');
} finally { await rm(dir,{recursive:true,force:true}); }
