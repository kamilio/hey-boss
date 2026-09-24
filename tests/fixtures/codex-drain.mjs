#!/usr/bin/env node
// Agent stays live until the test releases it; removal must not interrupt it.
import {createInterface} from 'node:readline';
import {existsSync, writeFileSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';
const session = randomUUID();
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
const cli = args => {
  const result = spawnSync(process.env.HEY_BOSS_TEST_CLI, ['issue', '--json', '--agent', `codex:${session}`, ...args], {encoding:'utf8'});
  if (result.status !== 0) throw Error(result.stderr + result.stdout);
};
for await (const line of createInterface({input:process.stdin})) {
  const message = JSON.parse(line);
  if (!message.method || message.id == null) continue;
  if (message.method === 'thread/start') {
    send({id:message.id,result:{thread:{id:session}}});
  } else if (message.method === 'turn/start') {
    const turn = randomUUID();
    const number = message.params.input[0].text.match(/issue view (\d+)/)[1];
    cli(['claim', number]);
    send({id:message.id,result:{turn:{id:turn}}});
    writeFileSync('agent-started', String(process.pid));
    while (!existsSync('release-agent')) await new Promise(resolve=>setTimeout(resolve,25));
    cli(['close', number]);
    send({method:'item/completed',params:{threadId:session,turnId:turn,item:{type:'agentMessage',text:JSON.stringify({status:'completed',summary:'Drain fixture finished normally.'})}}});
    send({method:'turn/completed',params:{threadId:session,turn:{id:turn,status:'completed'}}});
  } else {
    send({id:message.id,result:{}});
  }
}
