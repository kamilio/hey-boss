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
 if(q.method==='thread/start'){
   require('node:assert/strict').equal(q.params.config?.model_verbosity,'low');
   send({id:q.id,result:{thread:{id:threadId}}});
 }
 else if(q.method==='turn/start'){
   n++; const turnId='turn-'+n;
   send({id:q.id,result:{turn:{id:turnId}}});
   setTimeout(()=>{
     fs.writeFileSync('recorded',String(n));
     if(n===1)send({method:'item/completed',params:{threadId,turnId,item:{id:'user-'+n,type:'userMessage'}}});
     send({method:'item/started',params:{threadId,turnId,item:{id:'cmd',type:'commandExecution',command:'pwd'}}});
     send({id:'ask',method:'item/commandExecution/requestApproval',params:{threadId,turnId,command:'pwd'}});
   },150);
 } else if(q.id==='ask'){
   fs.appendFileSync('decisions',q.result.decision+'\\n');
   const method='mcpServer/elicitation/request', params={threadId,turnId,serverName:'fixture',mode:'form',message:'Allow access to the test page?',requestedSchema:{type:'object',properties:{}}};
   send({id:'other-chat',method,params:{...params,threadId:'other'}});
   send({id:'other-turn',method,params:{...params,turnId:'other'}});
   send({id:'form',method,params});
 } else if(q.id==='form'){
   fs.appendFileSync('forms',JSON.stringify(q.result)+'\\n');
   send({method:'item/completed',params:{threadId,turnId,item:{id:'cmd',type:'commandExecution',status:n===2?'completed':'declined'}}});
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
  let turns=0, starts=0, ends=0, messages=0, streamed=0, recorded=0;
  commands({turn:'Hello'});
  for await(const line of createInterface({input:child.stdout})){
    const e=JSON.parse(line);
    if(e.Session){
      assert.equal(await readFile(join(dir,'recorded'),'utf8').catch(()=>''),String(++recorded),'Only announce a thread after the provider records the user message');
      assert.equal(e.Session.id,'chat-fixture'); assert.equal(e.Session.accepted,true);
    }
    if(e.ToolStarted){assert.equal(e.ToolStarted.input.command,'pwd'); starts++;}
    if(e.Approval){
      assert.ok(['ask','form'].includes(e.Approval.payload.id),'Only show requests for the active conversation and turn');
      commands({decision:e.Approval.id,allow:turns===1});
    }
    if(e.ToolCompleted){assert.equal(e.ToolCompleted.failed,turns===0); ends++;}
    if(e.TextDelta){assert.equal(e.TextDelta.text,'reply '+(turns+1)); streamed++;}
    if(e.Message){assert.equal(e.Message.text,'reply '+(turns+1)); messages++;}
    if(e.TurnCompleted){assert.equal(e.TurnCompleted.output,'reply '+(++turns)); if(turns===1)commands({turn:'Hello'}); else child.stdin.end();}
  }
  clearTimeout(timeout);
  assert.equal(await exit,0); assert.deepEqual([turns,starts,ends,streamed,messages,recorded],[2,2,2,2,2,2]); assert.equal(await readFile(join(dir,'decisions'),'utf8'),'decline\naccept\n');
  assert.deepEqual((await readFile(join(dir,'forms'),'utf8')).trim().split('\n').map(JSON.parse),[{action:'decline',content:null},{action:'accept',content:{}}]);
  console.log('PASS workspace cwd, tool events, decline and allow, second turn, disconnect');
} finally { await rm(dir,{recursive:true,force:true}); }
