import test from 'node:test';
import assert from 'node:assert/strict';
import {HubStore} from './store.mjs';
import {createApp} from './index.mjs';

test('shared issue UI relays to the supervisor without retaining content on Fly',async t=>{
 const store=new HubStore();store.setIssueProjects([{id:'named:Atlas',name:'Atlas'}]);
 const key='x'.repeat(64),app=createApp({store,hubToken:key,secure:false,origin:'http://localhost'});
 const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));
 t.after(()=>{app.locals.close();server.close();store.close();});
 const base='http://127.0.0.1:'+server.address().port;
 const call=(path,body,headers={})=>fetch(base+path,{method:body===undefined?'GET':'POST',headers:{'Content-Type':'application/json',...headers},body:body===undefined?undefined:JSON.stringify(body)});
 assert.equal((await call('/issues')).status,401);
 assert.equal((await call('/api/bootstrap')).status,401);
 assert.equal((await call('/api/bridge/web')).status,401);
 const paired=await call('/api/pair',{code:store.pairing()});
 const phone={Cookie:paired.headers.get('set-cookie').split(';')[0]},bridge={Authorization:'Bearer '+key};
 assert.equal((await call('/api/action',{project:'named:Atlas',operation:{action:'list'}},{...phone,Origin:'https://evil.invalid'})).status,403);
 assert.equal((await call('/api/action',{project:'named:Atlas',host:'other',operation:{action:'view',number:1}},phone)).status,400);
 const reading=call('/api/bootstrap',undefined,phone);
 let queue;for(let i=0;i<30;i++){queue=(await(await call('/api/bridge/web',undefined,bridge)).json()).requests;if(queue.length)break;await new Promise(r=>setTimeout(r,10));}
 assert.equal(queue[0].kind,'bootstrap');
 await call('/api/bridge/web/'+queue[0].id+'/result',{ok:true,projects:[],actor:{id:'human:boss'}},bridge);
 assert.equal((await(await reading).json()).actor.id,'human:boss');
 const mutation={project:'named:Atlas',operation:{action:'edit',number:1,title:'Private title'},request_id:'stable-retry'};
 const writing=call('/api/action',mutation,phone);
 for(let i=0;i<30;i++){queue=(await(await call('/api/bridge/web',undefined,bridge)).json()).requests;if(queue.length)break;await new Promise(r=>setTimeout(r,10));}
 assert.deepEqual(queue[0].payload,mutation);
 await call('/api/bridge/web/'+queue[0].id+'/result',{ok:false,error:{code:'conflict',message:'Changed elsewhere'}},bridge);
 assert.equal((await(await writing).json()).error.code,'conflict');
 assert.equal((await(await call('/api/bridge/web',undefined,bridge)).json()).requests.length,0);
 assert.equal(store.db.prepare('SELECT count(*) AS n FROM artifact_requests').get().n,0);
 const cancelled=new AbortController();const pending=fetch(base+'/api/bootstrap',{headers:phone,signal:cancelled.signal}).catch(()=>{});
 for(let i=0;i<30;i++){queue=(await(await call('/api/bridge/web',undefined,bridge)).json()).requests;if(queue.length)break;await new Promise(r=>setTimeout(r,10));}
 cancelled.abort();await pending;await new Promise(r=>setTimeout(r,20));
 assert.equal((await(await call('/api/bridge/web',undefined,bridge)).json()).requests.length,0);
});
