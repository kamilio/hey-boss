#!/usr/bin/env node
// JSONL Codex fixture: delivery succeeds separately from authoritative resolution.
import assert from 'node:assert/strict';
import {appendFileSync, existsSync, readFileSync, writeFileSync} from 'node:fs';
import {randomUUID} from 'node:crypto';
import {spawnSync} from 'node:child_process';
import {createInterface} from 'node:readline';

let session = randomUUID(), goal = null, startedTurn = false;
const mode = existsSync('mode.txt') ? readFileSync('mode.txt', 'utf8').trim() : 'completed';
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
const cli = args => {
  const result = spawnSync(process.env.HEY_BOSS_TEST_CLI, args, {encoding: 'utf8'});
  assert.equal(result.status, 0, result.stderr + result.stdout);
  return JSON.parse(result.stdout);
};
for await (const line of createInterface({input: process.stdin})) {
  const msg = JSON.parse(line);
  appendFileSync('protocol.jsonl', JSON.stringify(msg) + '\n');
  if (!Object.hasOwn(msg, 'id') || !msg.method) continue;
  const params = msg.params ?? {};
  let result = {}, turn, issue;
  switch (msg.method) {
    case 'initialize': assert(params.capabilities.experimentalApi); break;
    case 'thread/start':
      assert.equal(params.ephemeral, false);
      writeFileSync('.codex-fixture-' + session + '.json', JSON.stringify({turns: []}));
      result = {thread: {id: session}}; break;
    case 'thread/resume': {
      if (mode === 'resume-unavailable') {
        send({id: msg.id, error: {code: -32600, message: 'Saved session is locked by another writer'}});
        continue;
      }
      session = params.threadId;
      assert(JSON.parse(readFileSync('.codex-fixture-' + session + '.json')).turns.length);
      result = {thread: {id: session}}; break;
    }
    case 'thread/goal/set':
      if (params.status === 'active') assert(startedTurn);
      goal = {...goal, ...params}; result = {goal}; break;
    case 'thread/goal/get':
      if (mode === 'partial-goal') goal = {...goal, status: 'complete'};
      result = {goal}; break;
    case 'turn/start': {
      startedTurn = true;
      const text = params.input[0].text;
      const path = '.codex-fixture-' + session + '.json';
      const saved = JSON.parse(readFileSync(path)); saved.turns.push(text);
      writeFileSync(path, JSON.stringify(saved));
      if (mode === 'offline-updates') {
        assert.equal(cli(['update', '--project', 'Offline worker QA', '--title', 'Issue finished', 'Worker finished without waiting for Mac', text, '--json']).status, 'pending');
      }
      const match = text.match(/issue view (\d+)/);
      if (!['unclaimed', 'delay-unclaimed', 'delay-model-start'].includes(mode)) {
        assert(match, text);
        assert.equal(process.env.HEY_BOSS_ISSUE_PROJECT, 'named:Worker fixture');
        issue = cli(['issue', '--json', '--agent', 'codex:' + session, 'view', match[1]]).issue;
        assert.equal(issue.number, Number(match[1]));
        cli(['issue', '--json', '--agent', 'codex:' + session, 'claim', match[1]]);
      }
      assert.equal(params.threadId, session);
      assert.deepEqual(params.outputSchema.required, ['status', 'summary']);
      turn = randomUUID(); result = {turn: {id: turn}}; break;
    }
  }
  send({id: msg.id, result});
  if (msg.method !== 'turn/start') continue;
  if (mode === 'disconnect') process.exit(9);
  if (['disconnect-after-text','failed-after-text','stale-turn-success','malformed-completion','terminal-error-event'].includes(mode)) {
    cli(['issue', '--json', '--agent', 'codex:' + session, 'close', String(issue.number)]);
    const eventTurn = mode === 'stale-turn-success' ? 'stale-turn' : turn;
    if (mode === 'stale-turn-success') send({method:'turn/started',params:{threadId:session,turn:{id:eventTurn}}});
    send({method:'item/completed',params:{threadId:session,turnId:eventTurn,item:{type:'agentMessage',text:mode==='malformed-completion'?'invalid completion report':JSON.stringify({status:'completed',summary:'Looks done, but the transport has not finished'})}}});
    if (mode === 'disconnect-after-text') process.exit(9);
    if (mode === 'terminal-error-event') {
      send({method:'error',params:{threadId:session,turnId:turn,willRetry:false,error:{message:'Model server is unavailable'}}});
      continue;
    }
    if (mode === 'stale-turn-success') send({method:'turn/completed',params:{threadId:session,turn:{id:eventTurn,status:'completed'}}});
    if (mode === 'malformed-completion') goal = {...goal,status:'complete'};
    send({method:'turn/completed',params:{threadId:session,turn:{id:turn,status:mode==='malformed-completion'?'completed':'failed',error:{message:'actual active turn failed'}}}});
    continue;
  }
  if (mode === 'delay-model-start') await new Promise(resolve => setTimeout(resolve, 6000));
  if (['delay-unclaimed', 'delay-model-start'].includes(mode)) {
    send({method: 'item/started', params: {threadId: session, item: {type: 'reasoning'}}}); continue;
  }
  if (mode === 'delay') continue;
  if (mode === 'transient-error-recovered') send({method:'error',params:{threadId:session,turnId:turn,willRetry:true,error:{message:'Temporary disconnect; retry in progress'}}});
  if (mode === 'approval') {
    send({id: 'approval-1', method: 'item/commandExecution/requestApproval', params: {threadId: session, command: 'synthetic privileged operation'}}); continue;
  }
  const outage = mode.startsWith('database-outage')
    ? 'Database service transport failed: Broken pipe (os error 32). Write outcome may be unknown; mutations are never automatically replayed.'
    : 'HTTP 504 from local hey-proxy: internal recovery time budget exhausted before a response could be forwarded.';
  if (mode.startsWith('database-outage') || mode.startsWith('proxy-outage')) {
    if (mode.endsWith('-rpc')) {
      send({method:'turn/completed',params:{threadId:session,turn:{id:turn,status:'failed',error:{message:outage}}}});
      continue;
    }
    if (!mode.endsWith('-summary')) send({method:'item/completed',params:{threadId:session,item:{type:'commandExecution',exitCode:1,aggregatedOutput:outage}}});
    if (!mode.endsWith('-recovered')) {
      send({method:'item/completed',params:{threadId:session,item:{type:'agentMessage',text:JSON.stringify({status:'blocked',summary:mode.endsWith('-summary')?outage:'Delivery cannot continue; saved work remains.'})}}});
      send({method:'turn/completed',params:{threadId:session,turn:{id:turn,status:'completed'}}});
      continue;
    }
  }
  if (['approval-outage-tool', 'approval-outage-recovered'].includes(mode)) {
    send({method: 'item/completed', params: {threadId: session, item: {type: 'mcpToolCall', status: 'failed', result: {content: [{type: 'text', text: 'Automatic approval review failed: unexpected status 404 Not Found: The model `wisp-alpha` does not exist or you do not have access to it. The action was not executed because automatic approval review could not be completed.'}]}}}});
  }
  if (mode.startsWith('approval-outage') && mode !== 'approval-outage-recovered') {
    const summary = mode === 'approval-outage-tool' ? 'Final verification cannot continue. Worktree and repairs retained.' : 'Automatic approval review still fails with HTTP 404 for missing `wisp-alpha`, preventing GitHub CI/review reads. Restore the approval service to finish notification and cleanup.';
    send({method: 'item/completed', params: {threadId: session, item: {type: 'agentMessage', text: JSON.stringify({status: 'blocked', summary})}}});
    send({method: 'turn/completed', params: {threadId: session, turn: {id: turn, status: 'completed'}}});
    continue;
  }
  const status = ['unclaimed', 'offline-updates', 'subtasks-completed', 'partial', 'partial-goal', 'approval-outage-recovered', 'database-outage-recovered', 'proxy-outage-recovered','transient-error-recovered'].includes(mode) ? 'completed' : mode;
  const workerArgs = existsSync('worker-args.json') ? JSON.parse(readFileSync('worker-args.json', 'utf8')) : [];
  const artifact = issue?.labels.some(label => ['task:plan', 'task:research'].includes(label));
  if (status === 'completed' && issue && !mode.startsWith('partial') && (!workerArgs.includes('--prs') || artifact)) {
    cli(['issue', '--json', '--agent', 'codex:' + session, 'close', String(issue.number), '--comment', 'All fixture requirements resolved and verified.']);
  }
  let text = JSON.stringify({status, summary: 'Implemented fixture. Meaningful checks passed.'});
  if (mode === 'partial-goal') {
    text = 'Pushed the partial fix. Genuine filter and citeproc engines remain unimplemented; leave the issue open.';
  }
  send({method: 'item/completed', params: {threadId: session, item: {type: 'agentMessage', text}}});
  send({method: 'turn/completed', params: {threadId: session, turn: {id: turn, status: 'completed'}}});
}
