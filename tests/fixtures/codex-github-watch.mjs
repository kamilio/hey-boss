#!/usr/bin/env node
// A real JSONL transport with synthetic model results and an actual task claim.
import assert from 'node:assert/strict';
import {appendFileSync, readFileSync, writeFileSync, existsSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {randomUUID} from 'node:crypto';

const session = randomUUID();
let turn, phase;
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
const record = value => appendFileSync('github-worker.jsonl', JSON.stringify({...value, session}) + '\n');
const status = text => JSON.parse(text.split('\n').find(line => line.startsWith('{"github_status":'))).github_status;
const finish = () => {
  send({method:'item/completed',params:{threadId:session,turnId:turn,item:{type:'agentMessage',text:JSON.stringify({status:'completed',summary:'Synthetic GitHub findings handled.'})}}});
  send({method:'turn/completed',params:{threadId:session,turn:{id:turn,status:'completed'}}});
};
for await (const line of createInterface({input:process.stdin})) {
  const message = JSON.parse(line);
  if (!Object.hasOwn(message,'id') || !message.method) continue;
  const params = message.params || {};
  let result = {};
  if (message.method === 'thread/start') result = {thread:{id:session}};
  if (message.method === 'thread/resume') throw Error('A new GitHub event must start a fresh session');
  if (message.method === 'turn/start') {
    assert(!turn, 'The active turn should be steered, not replaced');
    phase = existsSync('github-launches.txt') ? Number(readFileSync('github-launches.txt','utf8')) + 1 : 1;
    writeFileSync('github-launches.txt', String(phase));
    const initial = status(params.input[0].text);
    assert(initial.event);
    assert.equal(initial.monitoring,true);
    const claim = spawnSync(process.env.HEY_BOSS_TEST_CLI,['issue','--json','--agent','codex:'+session,'claim','1'],{encoding:'utf8'});
    assert.equal(claim.status,0,claim.stdout+claim.stderr);
    const claimed = JSON.parse(claim.stdout).issue;
    assert.equal(claimed.github_status.event,initial.event);
    record({type:'claim',phase,status:initial});
    turn = randomUUID();
    result = {turn:{id:turn}};
  }
  if (message.method === 'turn/steer') {
    assert.equal(phase,1);
    assert.equal(params.expectedTurnId,turn);
    assert(!params.input[0].text.includes('Boss added an instruction'), 'Automatic evidence must not impersonate a human instruction');
    const update = status(params.input[0].text);
    assert.equal(Object.values(update.prs)[0].evidence.complete,true);
    if (process.env.HEY_BOSS_TEST_REJECT_STEERING === '1') {
      record({type:'steer_rejected',phase,status:update});
      send({id:message.id,error:{code:-32601,message:'Steering is unsupported in this fixture'}});
      setTimeout(finish, 100);
      continue;
    }
    record({type:'steer',phase,status:update});
    result = {turnId:turn};
  }
  send({id:message.id,result});
  if (message.method === 'turn/steer' || (message.method === 'turn/start' && phase > 1)) finish();
}
