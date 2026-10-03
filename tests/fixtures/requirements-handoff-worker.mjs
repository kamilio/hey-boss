#!/usr/bin/env node
// Deterministic provider protocol fixture; all mutations use the public CLI.
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {spawnSync} from 'node:child_process';
import {createInterface} from 'node:readline';

const session = randomUUID();
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
const cli = args => {
  const result = spawnSync(process.env.HEY_BOSS_HANDOFF_TEST_BINARY,
    ['issue', '--json', '--agent', 'codex:' + session, ...args], {encoding:'utf8'});
  assert.equal(result.status, 0, result.stderr + result.stdout);
  return JSON.parse(result.stdout);
};
for await (const line of createInterface({input:process.stdin})) {
  const message = JSON.parse(line);
  if (!message.method || message.id === undefined) continue;
  if (message.method === 'thread/start') {
    send({id:message.id, result:{thread:{id:session}}});
  } else if (message.method === 'turn/start') {
    const number = message.params.input[0].text.match(/issue view (\d+)/)[1];
    const turn = randomUUID();
    cli(['claim', number]);
    const original = cli(['view', number]).issue.body;
    cli(['edit', number, '--body', 'Published handoff notes\n\n' + original]);
    cli(['ready', number, '--acknowledge-requirements']);
    cli(['assign', number, 'github']);
    send({id:message.id, result:{turn:{id:turn}}});
    send({method:'item/completed', params:{threadId:session, turnId:turn,
      item:{type:'agentMessage', text:JSON.stringify({status:'completed', summary:'Published delivery remains Ready for review.'})}}});
    send({method:'turn/completed', params:{threadId:session, turn:{id:turn, status:'completed'}}});
  } else {
    send({id:message.id, result:{}});
  }
}
