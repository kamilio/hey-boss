import test from 'node:test';
import assert from 'node:assert/strict';
import {HubStore} from './store.mjs';
const project={id:'named:Files',name:'Files'};
const upload=(id,data='AA==')=>({project:project.id,operation:{action:'attachment',operation:{command:'upload',target:{kind:'issue',id:'1'},name:'file.bin',data}},request_id:id});
test('paired attachment transport deduplicates writes, restricts commands and isolates downloads',()=>{
 const store=new HubStore();store.setIssueProjects([project]);
 try {
  const first=store.artifactRequest('phone',upload('file-1'));
  assert.deepEqual(store.artifactRequest('phone',upload('file-1')),first);
  assert.throws(()=>store.artifactRequest('phone',upload('file-1','AQ==')),/another request/);
  assert.throws(()=>store.artifactRequest('other',upload('file-1')),/another request/);
  for(const command of ['list','download']) {
   const read=store.artifactRequest('phone',{project:project.id,operation:{action:'attachment',operation:{command,id:'f-'+'a'.repeat(32),target:{kind:'issue',id:'1'}}}});
   store.finishArtifact(read.id,{ok:true,data:'AA=='});
   assert.throws(()=>store.artifactResult('other',read.id),/not found/);
  }
  assert.throws(()=>store.artifactRequest('phone',{...upload('file-2'),operation:{action:'attachment',operation:{command:'run'}}}),/artifact/);
  assert.throws(()=>store.artifactRequest('phone',{...upload('file-2'),host:'arbitrary'}),/artifact/);
  store.finishArtifact(first.id,{ok:true,attachment:{id:'stable'}});
  assert.deepEqual(store.artifactRequest('phone',upload('file-1')).result,{ok:true,attachment:{id:'stable'}});
 }finally{store.close();}
});
test('attachment uploads accept the full 10 MiB and bridge pages stay within the wire budget',()=>{
 const store=new HubStore();store.setIssueProjects([project]);
 try {
  const data=Buffer.alloc(10*1024*1024).toString('base64');
  store.artifactRequest('phone',upload('large-1',data));
  store.artifactRequest('phone',upload('large-2',data));
  const batch=store.pendingArtifacts();
  assert.equal(batch.length,1);
  assert.ok(Buffer.byteLength(JSON.stringify({requests:batch}))<16*1024*1024);
  assert.throws(()=>store.artifactRequest('phone',upload('large-3',Buffer.alloc(10*1024*1024+1).toString('base64'))),/10 MiB/);
 }finally{store.close();}
});

test('paired HTTP uploads accept full-size files and reject unauthenticated or cross-origin writes',async t=>{
 const {createApp}=await import('./index.mjs');
 const store=new HubStore();store.setIssueProjects([project]);
 const app=createApp({store,hubToken:'synthetic-bridge-key-'.repeat(4),secure:false,origin:'http://127.0.0.1'});
 const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));
 t.after(()=>{app.locals.close();server.close();store.close();});
 const base='http://127.0.0.1:'+server.address().port;
 const call=(body,headers={})=>fetch(base+'/api/artifact-requests',{method:'POST',headers:{'Content-Type':'application/json',...headers},body:JSON.stringify(body)});
 assert.equal((await call(upload('unauthenticated'))).status,401);
 const pair=await fetch(base+'/api/pair',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({code:store.pairing()})});
 const headers={Cookie:pair.headers.get('set-cookie').split(';')[0]};
 assert.equal((await call(upload('cross-origin'),{...headers,Origin:'https://evil.invalid'})).status,403);
 const response=await call(upload('full-size',Buffer.alloc(10*1024*1024).toString('base64')),headers);
 assert.equal(response.status,202);assert.equal((await response.json()).request.status,'pending');
 assert.equal(store.pendingArtifacts().length,1);
});
