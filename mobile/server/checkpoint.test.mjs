import test from 'node:test';
import assert from 'node:assert/strict';
import {HubStore} from './store.mjs';
import {createApp} from './index.mjs';
import {mkdtempSync,readdirSync,rmSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {spawn} from 'node:child_process';
import {createServer} from 'node:net';
import {fileURLToPath} from 'node:url';

test('supervisor checkpoints preserve pairing and accepted answers across a diskless hub restart',()=>{
 const first=new HubStore();const pair=first.pair(first.pairing());
 first.upsert({taskID:'decision',kind:'approval',options:['Approve'],title:'Decision',question:'Ready?'});
 first.resolve('decision','Approve','phone');first.setPreferences({mode:'always',awayAfterSeconds:600});
 const snapshot=first.snapshot();first.close();const restarted=new HubStore();
 try{
  restarted.restore(snapshot);
  assert.ok(restarted.device(pair.secret));assert.equal(restarted.get('decision').result,'Approve');
  assert.equal(restarted.preferences().mode,'always');
  assert.throws(()=>restarted.restore({...snapshot,devices:[{id:'invalid'}]}));
  assert.equal(restarted.get('decision').result,'Approve','invalid restores roll back atomically');
 }finally{restarted.close();}
});

test('successful phone mutations wait for durable supervisor acknowledgment',async t=>{
 const store=new HubStore(),key='x'.repeat(64);
 const app=createApp({store,hubToken:key,secure:false,origin:'http://localhost',checkpoint:true});
 const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));
 t.after(()=>{app.locals.close();server.close();store.close();});
 const base='http://127.0.0.1:'+server.address().port;
 const call=(path,body,headers={})=>fetch(base+path,{method:body===undefined?'GET':'POST',headers:{'Content-Type':'application/json',...headers},body:body===undefined?undefined:JSON.stringify(body)});
 const bridge={Authorization:'Bearer '+key};
 assert.equal((await call('/api/bridge/checkpoint')).status,401);
 assert.equal((await call('/api/pair',{code:'not-restored'})).status,503);
 await call('/api/bridge/checkpoint/restore',{snapshot:store.snapshot()},bridge);
 const code=store.pairing();let completed=false;
 const pairing=call('/api/pair',{code}).then(value=>{completed=true;return value;});
 await new Promise(r=>setTimeout(r,30));assert.equal(completed,false);
 const state=await(await call('/api/bridge/checkpoint',undefined,bridge)).json();
 assert.equal(state.snapshot.devices.length,1);
 await call('/api/bridge/checkpoint/ack',{epoch:'wrong',version:state.version},bridge);
 await new Promise(r=>setTimeout(r,20));assert.equal(completed,false);
 await call('/api/bridge/checkpoint/ack',{epoch:state.epoch,version:state.version},bridge);
 assert.equal((await pairing).status,200);
 assert.equal((await(await call('/api/bridge/checkpoint',undefined,bridge)).json()).snapshot,undefined,'unchanged state does not cross the network again');
 assert.equal((await call('/api/bridge/checkpoint/restore',{snapshot:store.snapshot()},bridge)).status,409,'a late restore cannot overwrite live answers');
});

test('the production entry point starts and stops without creating a user-data file',async t=>{
 const directory=mkdtempSync(join(tmpdir(),'hb-diskless-'));
 const probe=createServer();await new Promise(r=>probe.listen(0,'127.0.0.1',r));const port=probe.address().port;await new Promise(r=>probe.close(r));
 const child=spawn(process.execPath,[fileURLToPath(new URL('./index.mjs',import.meta.url))],{cwd:directory,env:{...process.env,PORT:String(port),HUB_TOKEN:'synthetic-local-test-key-000000000000',PUBLIC_ORIGIN:'http://127.0.0.1:'+port,VAPID_SUBJECT:'https://example.invalid',INSECURE_LOCAL:'1'},stdio:'ignore'});
 t.after(async()=>{child.kill('SIGTERM');if(child.exitCode===null)await new Promise(r=>child.once('exit',r));rmSync(directory,{recursive:true,force:true});});
 let response;for(let i=0;i<100;i++){try{response=await fetch('http://127.0.0.1:'+port+'/healthz');if(response.ok)break;}catch{}await new Promise(r=>setTimeout(r,20));}
 assert.equal(response?.status,200);assert.deepEqual(readdirSync(directory),[]);
 assert.equal((await fetch('http://127.0.0.1:'+port+'/api/tasks')).status,503,'cold startup refuses changes until supervisor restoration');
});
