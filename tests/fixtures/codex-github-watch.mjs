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
const status = () => {
  const view = spawnSync(process.env.HEY_BOSS_TEST_CLI,['issue','--json','--agent','codex:'+session,'view','1'],{encoding:'utf8'});
  assert.equal(view.status,0,view.stdout+view.stderr);
  return JSON.parse(view.stdout).issue.github_status;
};
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
    assert(!turn, 'Each watcher claim starts its own thread');
    phase = existsSync('github-launches.txt') ? Number(readFileSync('github-launches.txt','utf8')) + 1 : 1;
    writeFileSync('github-launches.txt', String(phase));
    assert(params.input[0].text.startsWith('Claim and implement `hey-boss issue view 1`.'));
    assert(!params.input[0].text.includes('hey-boss issue pr list 1'));
    assert(params.input[0].text.endsWith("The GitHub status changed."));
    const initial = status();
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
  assert.notEqual(message.method, 'turn/steer', 'Watcher events must wait for a new claim');
  send({id:message.id,result});
  if (message.method === 'turn/start') {
    if (phase > 1) finish();
    else { const timer = setInterval(() => { if (existsSync('release-first')) {clearInterval(timer); finish();} }, 50); }
  }
}
