#!/usr/bin/env node
import { writeFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const provider = process.env.HEY_BOSS_FIXTURE_PROVIDER;
const args = process.argv.slice(2);
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
let session = '00000000-0000-0000-0000-000000000001';
let file;
let turn = 'fixture-turn';
let streaming = false;
let turnNumber = 0;
let goalTurns = 0;
let output = '';
const queued = [];
if (provider === 'pi') {
  file = args.includes('--session') ? args[args.indexOf('--session') + 1] : join(mkdtempSync(join(tmpdir(), 'hey-boss-pi-fixture-')), session + '.jsonl');
  writeFileSync(file, JSON.stringify({ type: 'session', id: session }) + '\n');
}
function complete(interrupted = false) {
  streaming = false;
  if (provider === 'codex') {
    send({method:'item/completed',params:{threadId:session,item:{type:'agentMessage',text:output}}});
    send({method:'turn/completed',params:{threadId:session,turn:{id:turn,status:interrupted ? 'interrupted' : 'completed'}}});
  } else if (provider === 'claude') {
    send({type:'result',session_id:session,subtype:interrupted ? 'error_during_execution' : 'success',is_error:interrupted,result:output,errors:interrupted ? ['interrupted'] : []});
    if (queued.length) prompt(queued.shift());
  } else {
    send({type:'message_end',message:{role:'assistant',content:[{type:'text',text:output}],stopReason:interrupted ? 'aborted' : 'stop'}});
    send({type:'agent_end',messages:[],willRetry:false});
    send({type:'agent_settled'});
  }
}
function prompt(text) {
  streaming = true;
  output = text;
  if (text === 'owned identity') output = process.env.HEY_BOSS_AGENT_ID ?? 'missing';
  if (text.includes('goal fixture')) {
    goalTurns += 1;
    if (goalTurns === 2) output = JSON.stringify({status:'completed',summary:'Goal fixture verified'});
  }
  if (provider === 'claude') send({type:'system',subtype:'init',session_id:session});
  if (provider === 'pi') send({type:'agent_start'});
  if (provider === 'codex') send({method:'item/agentMessage/delta',params:{threadId:session,delta:text}});
  if (provider === 'claude') send({type:'stream_event',session_id:session,event:{type:'content_block_delta',delta:{type:'text_delta',text}}});
  if (provider === 'pi') send({type:'message_update',assistantMessageEvent:{type:'text_delta',delta:text}});
  if (text.startsWith('hold')) return;
  if (text.startsWith('queued goal')) { output = JSON.stringify({status:'completed',summary:'Initial turn verified'}); setTimeout(() => complete(), 300); return; }
  if (text === 'queued turns') { setTimeout(() => complete(), 300); return; }
  if (text === 'retry' && provider === 'pi') {
    send({type:'agent_end',messages:[],willRetry:true});
    setTimeout(() => { output = 'retry settled'; complete(); }, 100);
    return;
  }
  if (text === 'input' && provider === 'pi') {
    send({type:'extension_ui_request',id:'input-1',method:'select',title:'Fixture input',options:['one','two']});
    return;
  }
  if (text === 'malformed') { process.stdout.write('{invalid JSON}\n'); return; }

  if (text === 'approval') {
    if (provider === 'codex') send({id:'foreign',method:'item/commandExecution/requestApproval',params:{threadId:'other-session',turnId:turn}});
    if (provider === 'codex') send({id:'approval-1',method:'item/commandExecution/requestApproval',params:{threadId:session,turnId:turn,command:'fixture command'}});
    else send({type:'control_request',request_id:'approval-1',request:{subtype:'can_use_tool',tool_name:'Bash',input:{command:'fixture command'}}});
    return;
  }
  complete();
}
let buffered = '';
for await (const chunk of process.stdin) {
  buffered += chunk.toString();
  let newline;
  while ((newline = buffered.indexOf('\n')) >= 0) {
  const line = buffered.slice(0, newline); buffered = buffered.slice(newline + 1);
  const r = JSON.parse(line);
  if (provider === 'codex') {
    const p = r.params ?? {};
    if (!r.method) {
      if (r.id === 'approval-1') {
        if (r.result.decision !== 'decline') throw new Error('expected explicit decline');
        complete();
      }
      continue;
    }
    if (r.method === 'initialized') continue;
    let result = {};
    if (r.method === 'thread/start' || r.method === 'thread/resume') result = {thread:{id:session}};
    if (r.method === 'turn/start') { turn = 'fixture-turn-' + (++turnNumber); result = {turn:{id:turn}}; }
    if (r.method === 'turn/steer') result = {turnId:turn};
    if (r.method === 'thread/read') result = {thread:{id:session,status:{type:streaming?'active':'idle'},canAcceptDirectInput:streaming}};
    send({id:r.id,result});
    if (r.method === 'turn/start') prompt(p.input[0].text);
    if (r.method === 'turn/interrupt') complete(true);
  } else if (provider === 'claude') {
    if (r.type === 'control_request') {
      send({type:'control_response',response:{subtype:'success',request_id:r.request_id,response:{}}});
      if (r.request.subtype === 'interrupt') complete(true);
    } else if (r.type === 'user') {
      if (!streaming) prompt(r.message.content); else queued.push(r.message.content);
    } else if (r.type === 'control_response') {
      if (r.response.response.behavior !== 'deny') throw new Error('expected explicit denial');
      complete();
    }
  } else {
    if (r.type === 'extension_ui_response') {
      if (r.cancelled !== true) throw new Error('expected explicit input cancellation');
      output = 'input cancelled'; complete(); continue;
    }
    let data;
    if (r.type === 'get_state') data = {sessionId:session,sessionFile:file,isStreaming:streaming};
    send({id:r.id,type:'response',command:r.type,success:true,data});
    if (r.type === 'prompt') prompt(r.message);
    if (r.type === 'abort') complete(true);
  }
}
}
