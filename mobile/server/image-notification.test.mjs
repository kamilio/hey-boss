import test from 'node:test';
import assert from 'node:assert/strict';
import {HubStore} from './store.mjs';

test('image notifications keep snapshots in detail and history without bloating inbox polling', () => {
 const store = new HubStore();
 const attachment = {name:'Preview.png',mime:'image/png',data:'a'.repeat(1024 * 1024)};
 try {
  store.upsert({taskID:'image',kind:'update',title:'Preview',question:'New layout',description:'New layout',attachment,options:[]});
  assert.equal(store.summaries()[0].attachment, undefined);
  assert.deepEqual(store.get('image').attachment, attachment);
  store.open('image');
  assert.equal(store.get('image').status, 'ok');
  const restored = new HubStore();
  try {
   restored.restore(store.snapshot());
   assert.deepEqual(restored.get('image').attachment, attachment);
   assert.equal(restored.summaries()[0].attachment, undefined);
  } finally {restored.close();}
 } finally {store.close();}
});
