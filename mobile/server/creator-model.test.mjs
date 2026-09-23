import test from 'node:test';
import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
const origin = createRequire(import.meta.url)('../../src/issues/web/origin.js');

test('creator labels use captured models, preserving human and historical attribution', () => {
  const name = id => id === 'human:boss' ? 'Kamil' : 'Codex · aaaaaaaa';
  assert.equal(origin.creator({created_by:'codex:a',origin:{kind:'codex',model:'gpt-6-astra'}},name), 'Codex · gpt-6-astra');
  assert.equal(origin.creator({created_by:'codex:a',origin:null},name), 'Codex · aaaaaaaa');
  assert.equal(origin.creator({created_by:'human:boss',origin:{kind:'human'}},name), 'Kamil');
  for (const model of ['',null,{},'\u001b[31m','x'.repeat(257)]) {
    assert.equal(origin.creator({created_by:'codex:a',origin:{kind:'codex',model}},name), 'Codex · aaaaaaaa');
  }
});

test('origin cards escape model names and keep the original conversation link', () => {
  global.document = {documentElement:{dataset:{}}};
  const context = {actor_id:'codex:exact',kind:'codex',model:'custom/<model>',session_id:'exact',host:'mac',cwd:'/work'};
  const card = origin.card(context,'Project');
  assert.match(card,/Codex · custom\/&lt;model&gt;/);
  assert.match(card,/run=session%3Aexact/);
  assert.match(card,/<dd>exact<\/dd>/);
  assert.doesNotMatch(card,/<model>/);
});
