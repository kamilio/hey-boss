import test from 'node:test';
import assert from 'node:assert/strict';
import {loadIssueDraft,saveIssueDraft,issuePayload,taskKind,taskLabels} from '../src/issue-draft.js';
test('artifact task intent survives offline retries and matches the shared editor',async()=>{
 const {createRequire}=await import('node:module');
 const shared=createRequire(import.meta.url)('../../src/issues/web/quick-issue.js');
 const storage={data:null,getItem(){return this.data;},setItem(key,value){this.data=value;}};
 for(const kind of ['implement','plan']){
  const labels=taskLabels(['ready','task:plan','task:research','ready'],kind);
  assert.deepEqual(labels,shared.taskLabels(['ready','task:plan','task:research','ready'],kind));
  const draft={...loadIssueDraft(storage),task:kind,labels:labels.join(', '),submitted:true};
  saveIssueDraft(storage,draft);
  const restored=loadIssueDraft(storage);
  assert.equal(restored.requestID,draft.requestID);
  assert.equal(taskKind(issuePayload(restored).labels),kind);
  assert.equal(shared.taskKind(labels),kind);
 }
});
test('a pending draft reloads with the same immutable retry ID and complete payload',()=>{
 const storage={data:null,getItem(){return this.data;},setItem(key,value){this.data=value;}};
 const draft={...loadIssueDraft(storage),project:'project',title:'Retain me',body:'First\nSecond',labels:'ready, phone',submitted:true};saveIssueDraft(storage,draft);
 assert.deepEqual(loadIssueDraft(storage),draft);assert.deepEqual(issuePayload(loadIssueDraft(storage)),{requestID:draft.requestID,project:'project',title:'Retain me',body:'First\nSecond',labels:['ready','phone']});
});
test('unavailable persistent storage fails before submission',()=>{
 assert.throws(()=>saveIssueDraft({setItem(){throw Error('Storage full');}},loadIssueDraft({getItem(){return null;}})),/Storage full/);
});
test('older pending drafts retain their exact payload after an upgrade',()=>{
 const legacy={project:'project',title:'Pending',body:'Notes',labels:'ready, ready, task:plan, task:research',requestID:'same-retry-id',submitted:true};
 const restored=loadIssueDraft({getItem(){return JSON.stringify(legacy);}});
 assert.deepEqual(issuePayload(restored),{requestID:'same-retry-id',project:'project',title:'Pending',body:'Notes',labels:['ready','ready','task:plan','task:research']});
});
test('removed Research choice preserves pending retries and converts editable drafts to Plan',()=>{
 const legacy={project:'project',title:'Pending',body:'Notes',labels:'ready, ready',task:'research',requestID:'same-retry-id',submitted:true};
 assert.deepEqual(issuePayload(legacy).labels,['ready','task:research']);
 assert.equal(taskKind(issuePayload(legacy).labels),'plan');
 assert.deepEqual(issuePayload({...legacy,submitted:false}).labels,['ready','task:plan']);
});
