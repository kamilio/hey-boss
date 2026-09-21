import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {HubStore} from './store.mjs';
import {createApp} from './index.mjs';
const project={id:'github.com/kamilio/hey-boss',name:'hey-boss'};
const draft={requestID:'mobile-123',project:project.id,title:'Phone issue',body:'Description',labels:['ready']};
test('project names are unique destinations and accept name-based submissions',()=>{
 const store=new HubStore();
 try{
  store.setIssueProjects([project,{id:'named:hey-boss',name:'HEY-BOSS'}]);
  assert.equal(store.issueProjects().length,1);
  assert.equal(store.issueProjects()[0].name_collisions.length,1);
  const byName={...draft,project:'hey-boss'};
  const created=store.createIssue('phone',byName);
  assert.equal(created.project,'hey-boss');
  assert.deepEqual(store.createIssue('phone',byName),created);
  assert.equal(store.createIssue('phone',{...draft,requestID:'compatibility-id'}).project,project.id);
 }finally{store.close();}
});
test('offline creations survive restart and lost acknowledgments without duplicates',()=>{
 const directory=mkdtempSync(join(tmpdir(),'hb-issues-'));let store=new HubStore(join(directory,'hub.db'));
 try{
  store.setIssueProjects([project]);const first=store.createIssue('phone',draft);assert.equal(first.status,'pending');store.close();store=new HubStore(join(directory,'hub.db'));
  assert.equal(store.pendingIssues().length,1);assert.deepEqual(store.createIssue('phone',draft),first);
  assert.throws(()=>store.createIssue('phone',{...draft,title:'Changed'}),{status:409});
  store.finishIssue(draft.requestID,{status:'synced',number:42});store.finishIssue(draft.requestID,{status:'error',error:'Late error'});
  assert.equal(store.issueCreations('phone')[0].number,42);assert.equal(store.pendingIssues().length,0);assert.equal(store.createIssue('phone',draft).status,'synced');
 }finally{store.close();rmSync(directory,{recursive:true,force:true});}
});
test('invalid drafts and unknown projects are rejected; authoritative errors retain all content',()=>{
 const store=new HubStore();store.setIssueProjects([project]);
 for(const changes of [{title:' '},{title:'🌍'.repeat(129)},{title:'bad\nline'},{labels:['x'.repeat(65)]},{body:'x'.repeat(1048577)},{project:'unknown'},{requestID:'bad/id'}])assert.throws(()=>store.createIssue('phone',{...draft,...changes}),{status:400});
 store.createIssue('phone',draft);store.finishIssue(draft.requestID,{status:'error',error:'Project was hidden. Choose another project.'});
 const row=store.issueCreations('phone')[0];assert.equal(row.body,draft.body);assert.deepEqual(row.labels,draft.labels);assert.equal(row.status,'error');store.close();
});
test('issue APIs require pairing or bridge credentials and isolate devices',async t=>{
 const store=new HubStore(),key='x'.repeat(64);const app=createApp({store,hubToken:key,secure:false,origin:'http://127.0.0.1'});const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));t.after(()=>{app.locals.close();server.close();store.close();});
 const base='http://127.0.0.1:'+server.address().port;
 const call=async(path,body,headers={})=>{const r=await fetch(base+'/api'+path,{method:body===undefined?'GET':'POST',headers:{'Content-Type':'application/json',...headers},body:body===undefined?undefined:JSON.stringify(body)});return {status:r.status,value:await r.json()};};
 assert.equal((await call('/issues')).status,401);assert.equal((await call('/bridge/issues')).status,401);
 const bridge={Authorization:'Bearer '+key};assert.equal((await call('/bridge/issue-projects',{projects:[project]},bridge)).status,200);
 const pair=async()=>{const code=store.pairing();const r=await fetch(base+'/api/pair',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({code})});return {Cookie:r.headers.get('set-cookie').split(';')[0]};};const phone=await pair(),other=await pair();
 assert.equal((await call('/issues',draft,phone)).status,202);assert.equal((await call('/issues',draft,other)).status,409);
 assert.equal((await call('/issues',undefined,other)).value.creations.length,0);
 assert.equal((await call('/issues/mobile-123',undefined,other)).status,404);
 assert.equal((await call('/issues/mobile-123',undefined,phone)).value.creation.body,draft.body);
 assert.equal((await call('/issues',undefined,phone)).value.creations[0].body,undefined);
 assert.equal((await call('/bridge/issues',undefined,bridge)).value.creations.length,1);
 assert.equal((await call('/bridge/issues/mobile-123/result',{status:'synced',number:42},bridge)).status,200);
 assert.equal((await call('/issues',undefined,phone)).value.creations[0].number,42);
 assert.equal((await call('/issues',{...draft,requestID:'new'}, {...phone,Origin:'https://evil.invalid'})).status,403);
});
