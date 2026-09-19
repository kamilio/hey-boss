import test from 'node:test';
import assert from 'node:assert/strict';
import {loadIssueDraft,saveIssueDraft,issuePayload} from '../src/issue-draft.js';
test('a pending draft reloads with the same immutable retry ID and complete payload',()=>{
 const storage={data:null,getItem(){return this.data;},setItem(key,value){this.data=value;}};
 const draft={...loadIssueDraft(storage),project:'project',title:'Retain me',body:'First\nSecond',labels:'ready, phone',submitted:true};saveIssueDraft(storage,draft);
 assert.deepEqual(loadIssueDraft(storage),draft);assert.deepEqual(issuePayload(loadIssueDraft(storage)),{requestID:draft.requestID,project:'project',title:'Retain me',body:'First\nSecond',labels:['ready','phone']});
});
test('unavailable persistent storage fails before submission',()=>{
 assert.throws(()=>saveIssueDraft({setItem(){throw Error('Storage full');}},loadIssueDraft({getItem(){return null;}})),/Storage full/);
});
