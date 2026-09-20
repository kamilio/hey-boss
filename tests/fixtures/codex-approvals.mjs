#!/usr/bin/env node
// Synthetic Codex app-server: records exact callback replies, never runs approved tools.
import readline from 'node:readline';
import {appendFileSync, readFileSync} from 'node:fs';
import {randomUUID} from 'node:crypto';
import {execFileSync} from 'node:child_process';
const threadId = randomUUID(), turnId = randomUUID();
const mode = readFileSync('mode.txt', 'utf8').trim();
// Allow a just-written cancellation reply to be recorded before fixture shutdown.
process.on('SIGTERM', () => setTimeout(() => process.exit(0), 50));
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
let outstanding = 0, startId;
const complete = () => {
  if (startId != null) send({id:startId,result:{turn:{id:turnId}}});
  send({method:'item/completed',params:{threadId,item:{type:'agentMessage',text:JSON.stringify({status:'completed',summary:'Explicit decisions received in the original session.'})}}});
  send({method:'turn/completed',params:{threadId,turn:{id:turnId,status:'completed'}}});
};
for await (const line of readline.createInterface({input:process.stdin})) {
  const message = JSON.parse(line);
  appendFileSync('protocol.jsonl', line + '\n');
  if (!message.method) {
    if (--outstanding === 0) complete();
    continue;
  }
  if (message.method === 'thread/start') {
    send({id:message.id,result:{thread:{id:threadId}}});
  } else if (message.method === 'turn/start') {
    execFileSync(process.env.HEY_BOSS_TEST_CLI, ['issue','--agent','codex:'+threadId,'claim','1'], {stdio:'ignore'});
    if (mode === 'ack-race') startId = message.id;
    else send({id:message.id,result:{turn:{id:turnId}}});
    const count = mode === 'concurrent' ? 2 : 1;
    outstanding = count;
    for (let i=0; i<count; i++) {
      const itemId = count === 2 ? 'shared-item' : 'item-'+i;
      const params = {threadId:mode==='foreign' ? randomUUID() : threadId,turnId,itemId,cwd:'/synthetic/repo',reason:'Review the exact requested action.'};
      let method = 'item/commandExecution/requestApproval';
      if (mode === 'files') {
        method = 'item/fileChange/requestApproval';
        send({method:'item/started',params:{threadId,item:{id:itemId,type:'fileChange',changes:[{path:'/synthetic/repo/example.rs',diff:'-old\n+new'}]}}});
        params.grantRoot='/synthetic/repo';
      } else if (mode === 'permissions') {
        method = 'item/permissions/requestApproval';
        params.permissions={network:{enabled:true},fileSystem:{write:['/synthetic/repo']}};
      } else if (mode === 'network') {
        params.networkApprovalContext={host:'registry.example.com',protocol:'https'};
      } else {
        params.command='cargo test '+(i ? '--lib' : '--offline');
      }
      send({id:'approval-'+i,method,params});
      if (mode === 'resolved') {
        send({method:'serverRequest/resolved',params:{threadId,requestId:'approval-'+i}});
        setTimeout(complete, 300);
      }
    }
  } else {
    send({id:message.id,result:{}});
  }
}
