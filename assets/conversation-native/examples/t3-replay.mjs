#!/usr/bin/env node
import {readFileSync, appendFileSync} from 'node:fs';
const file = name => new URL(name, import.meta.url);
const [op, link] = process.argv.slice(2);
if (link !== 't3code://threads/00000000-0000-0000-0000-000000000001/simulated') throw Error('Wrong attached thread');
if (op === 'show') console.log(readFileSync(file('thread.json'), 'utf8'));
else if (op === 'answer') {
  const answers = JSON.parse(readFileSync(0, 'utf8'));
  if (process.argv[4] !== 'scope-request' || answers.scope !== 'Reconnect retries only') throw Error('Wrong question or answer');
  appendFileSync(file('sent.jsonl'), JSON.stringify({op,answers})+'\n');
  console.log(JSON.stringify({delivery:'answered'}));
} else if (['send','queue'].includes(op)) {
  appendFileSync(file('sent.jsonl'), JSON.stringify({op, text:readFileSync(0, 'utf8')})+'\n');
  console.log(JSON.stringify({delivery:op === 'queue' ? 'queued' : 'steered',messageId:'simulated-message'}));
} else throw Error('Unexpected T3 operation');
