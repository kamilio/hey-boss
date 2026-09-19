import {test} from 'node:test';
import assert from 'node:assert/strict';
import {HubStore} from './store.mjs';
import {createApp} from './index.mjs';
const task={taskID:'request-1',kind:'approval',title:'Ship the migration?',question:'All replicas have caught up.',description:'The migration is ready.',options:['Approve','Reject'],project:'Atlas'};
test('automatic pushes give the Mac time to open an update and recheck activity before delivery',async()=>{
 const store=new HubStore();let now=1000000;const sent=[];
 const {id}=store.pair(store.pairing());store.db.prepare('UPDATE devices SET subscription=? WHERE id=?').run('{}',id);
 store.presence({idleSeconds:600,unavailable:false},now);
 store.upsert({...task,kind:'update',options:[]});store.enqueue({id:task.taskID},now);
 const app=createApp({store,now:()=>now,hubToken:'x'.repeat(64),push:{setVapidDetails(){},async sendNotification(_,body){sent.push(JSON.parse(body));}},vapid:{subject:'mailto:test@example.invalid',publicKey:'public',privateKey:'private'}});
 await app.locals.pump();now+=29999;await app.locals.pump();assert.equal(sent.length,0);
 store.resolve(task.taskID,null,'mac');now++;await app.locals.pump();assert.equal(sent.length,0);
 for(const name of ['first','second']){store.upsert({...task,taskID:name});store.enqueue({id:name},now);}
 now+=30000;store.presence({idleSeconds:0,unavailable:false},now);await app.locals.pump();assert.equal(sent.length,0);
 store.presence({idleSeconds:600,unavailable:true},now);await app.locals.pump();assert.equal(sent.length,2);
 store.close();
});
test('a Mac answer arriving during another push suppresses the next queued push',async()=>{
 const store=new HubStore();let now=1000000;const sent=[];
 const {id}=store.pair(store.pairing());store.db.prepare('UPDATE devices SET subscription=? WHERE id=?').run('{}',id);
 for(const name of ['first','second']){store.upsert({...task,taskID:name});store.enqueue({id:name},now);}
 now+=30000;store.presence({idleSeconds:600,unavailable:true},now);
 const app=createApp({store,now:()=>now,hubToken:'x'.repeat(64),push:{setVapidDetails(){},async sendNotification(_,body){sent.push(JSON.parse(body));store.resolve('second','Approve','mac');}},vapid:{subject:'mailto:test@example.invalid',publicKey:'public',privateKey:'private'}});
 await app.locals.pump();assert.deepEqual(sent.map(x=>x.id),['first']);store.close();
});
test('automatic routing requires lock or sustained disconnect and ignores inactivity; preferences persist',()=>{
 const store=new HubStore();let now=1000000;
 assert.equal(store.routing(now).notifyPhone,false);assert.equal(store.routing(now).macState,'unknown');
 store.presence({idleSeconds:0,unavailable:false},now);
 assert.equal(store.routing(now).notifyPhone,false);
 store.presence({idleSeconds:119,unavailable:false},now);
 assert.equal(store.routing(now).macState,'active');assert.equal(store.routing(now+1000).notifyPhone,false);
 store.presence({idleSeconds:604800,unavailable:false},now);assert.equal(store.routing(now).notifyPhone,false);
 store.presence({idleSeconds:0,unavailable:true},now);assert.equal(store.routing(now).macState,'locked');
 store.presence({idleSeconds:0,unavailable:false},now);assert.equal(store.routing(now+30001).notifyPhone,false);assert.equal(store.routing(now+120001).macState,'offline');assert.equal(store.routing(now+120001).notifyPhone,true);
 store.setPreferences({mode:'off',awayAfterSeconds:300});assert.equal(store.routing(now+30001).notifyPhone,false);
 store.setPreferences({mode:'always',awayAfterSeconds:60});assert.equal(store.routing(now).notifyPhone,true);
 assert.throws(()=>store.setPreferences({mode:'automatic',awayAfterSeconds:0}),{status:400});
 assert.throws(()=>store.presence({idleSeconds:-1,unavailable:false}),{status:400});store.close();
});
test('trusted input inactivity requires a minute of continuous confirmation and resets on input or missing samples',()=>{
 const store=new HubStore();let now=1000000;
 store.presence({idleSeconds:10000,idleReliable:false,unavailable:false},now);assert.equal(store.routing(now).notifyPhone,false);
 const sample=idle=>store.presence({idleSeconds:idle,idleReliable:true,unavailable:false},now);
 sample(601);assert.equal(store.routing(now).macState,'confirming');
 now+=60000;assert.equal(store.routing(now).notifyPhone,false); // An old sample cannot confirm itself.
 sample(661);assert.equal(store.routing(now).notifyPhone,false); // Missing samples reset confirmation.
 for(let i=0;i<12;i++){now+=5000;sample(666+i*5);}
 assert.equal(store.routing(now).macState,'away');assert.equal(store.routing(now).notifyPhone,true);
 sample(0);assert.equal(store.routing(now).macState,'active');assert.equal(store.routing(now).notifyPhone,false);
 sample(601);now+=5000;store.presence({idleSeconds:601,idleReliable:false,unavailable:false},now);assert.equal(store.routing(now).notifyPhone,false);
 store.close();
});
test('active Mac defers pushes, pending decisions reach an away user once, and resolved work is suppressed',async()=>{
 const store=new HubStore();let now=1000000;const sent=[];
 const {id}=store.pair(store.pairing());store.db.prepare('UPDATE devices SET subscription=? WHERE id=?').run('{}',id);
 store.upsert({...task,createdAt:now/1000});store.enqueue({id:task.taskID,title:task.title},now);
 const app=createApp({store,now:()=>now,hubToken:'x'.repeat(64),push:{setVapidDetails(){},async sendNotification(subscription,body){sent.push(JSON.parse(body));}},vapid:{subject:'mailto:test@example.invalid',publicKey:'public',privateKey:'private'}});
 store.presence({idleSeconds:0,unavailable:false},now);await app.locals.pump();assert.equal(sent.length,0);
 assert.equal(store.db.prepare('SELECT attempts FROM outbox').get().attempts,0);
 now+=30000;store.presence({idleSeconds:130,unavailable:true},now);await app.locals.pump();await app.locals.pump();assert.equal(sent.length,1);
 store.upsert({...task,taskID:'handled'});store.enqueue({id:'handled'});store.resolve('handled','Approve','mac');await app.locals.pump();assert.equal(sent.length,1);
 store.upsert({...task,taskID:'old-update',kind:'update',options:[],createdAt:now/1000-1801});store.enqueue({id:'old-update'});await app.locals.pump();assert.equal(sent.length,1);
 store.close();
});
test('one accepted answer, with immutable terminal state and durable delivery acknowledgement',()=>{
 const store=new HubStore();store.upsert(task);const phone=store.resolve(task.taskID,'Approve','phone');
 assert.equal(phone.status,'ok');assert.throws(()=>store.resolve(task.taskID,'Reject','mac'),{status:409});
 store.upsert({...task,title:'Retry must not reset the answer'});assert.equal(store.get(task.taskID).result,'Approve');
 assert.equal(store.terminal().length,1);store.ack(task.taskID,phone.version-1);assert.equal(store.terminal().length,1);store.ack(task.taskID,phone.version);assert.equal(store.terminal().length,0);store.close();
});
test('an away backlog is summarized once instead of producing a notification burst',async()=>{
 const store=new HubStore();store.setPreferences({mode:'always',awayAfterSeconds:120});const {id}=store.pair(store.pairing());store.db.prepare('UPDATE devices SET subscription=? WHERE id=?').run('{}',id);
 for(let i=0;i<5;i++){store.upsert({...task,taskID:'backlog-'+i});store.enqueue({id:'backlog-'+i,title:'Decision'});}
 const sent=[];const app=createApp({store,hubToken:'x'.repeat(64),push:{setVapidDetails(){},async sendNotification(subscription,body){sent.push(JSON.parse(body));}},vapid:{subject:'mailto:test@example.invalid',publicKey:'public',privateKey:'private'}});
 await app.locals.pump();await app.locals.pump();assert.equal(sent.length,1);assert.equal(sent[0].id,'inbox');assert.equal(sent[0].taskIDs.length,5);assert.equal(store.db.prepare('SELECT COUNT(*) AS n FROM outbox').get().n,0);store.close();
});
test('questions validate exact answers and cancellation prevents a later answer',()=>{
 const store=new HubStore();store.upsert({...task,kind:'prompt',options:[]});assert.throws(()=>store.resolve(task.taskID,' ','phone'),{status:400});
 const result='First exact line\nSecond line 🌍';assert.equal(store.resolve(task.taskID,result,'mac').result,result);store.upsert({...task,taskID:'cancel'});store.resolve('cancel',null,'mac',true);assert.throws(()=>store.resolve('cancel','Approve','phone'),{status:409});store.close();
});
test('pairing is single use, server state is private, and request origins are checked',async t=>{
 const store=new HubStore();const key='x'.repeat(64);const app=createApp({store,hubToken:key,origin:'http://127.0.0.1',secure:false});const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));t.after(()=>{app.locals.close();server.close();store.close();});const base='http://127.0.0.1:'+server.address().port;
 assert.equal((await fetch(base+'/api/tasks')).status,401);
 const auth={Authorization:'Bearer '+key,'Content-Type':'application/json'};
 const code=(await (await fetch(base+'/api/bridge/pair-code',{method:'POST',headers:auth,body:'{}'})).json()).code;
 const pair=await fetch(base+'/api/pair',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({code})});assert.equal(pair.status,200);const cookie=pair.headers.get('set-cookie').split(';')[0];
 assert.equal((await fetch(base+'/api/pair',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({code})})).status,401);
 assert.equal((await fetch(base+'/api/bridge/tasks/request-1',{method:'PUT',headers:auth,body:JSON.stringify(task)})).status,200);
 const list=await (await fetch(base+'/api/tasks',{headers:{Cookie:cookie}})).json();assert.equal(list.tasks.length,1);
 assert.equal((await fetch(base+'/api/tasks/request-1/resolve',{method:'POST',headers:{Cookie:cookie,Origin:'https://evil.invalid','Content-Type':'application/json'},body:'{"result":"Approve"}'})).status,403);
 const calls=await Promise.all(['phone','mac'].map(actor=>fetch(base+'/api/'+(actor==='mac'?'bridge/':'')+'tasks/request-1/resolve',{method:'POST',headers:actor==='mac'?auth:{Cookie:cookie,'Content-Type':'application/json'},body:'{"result":"Approve"}'})));
 assert.deepEqual(calls.map(r=>r.status).sort(),[200,409]);
});
test('expired endpoints are removed and resolved tasks are never pushed from the outbox',async()=>{
 const store=new HubStore();store.setPreferences({mode:'always',awayAfterSeconds:120});const {id}=store.pair(store.pairing());store.db.prepare('UPDATE devices SET subscription=? WHERE id=?').run(JSON.stringify({endpoint:'https://a.push.apple.com/test'}),id);store.upsert(task);store.enqueue({id:task.taskID,title:'Test'});let sent=0;
 const app=createApp({store,hubToken:'x'.repeat(64),push:{setVapidDetails(){},async sendNotification(){sent++;throw {statusCode:410};}},vapid:{subject:'mailto:test@example.invalid',publicKey:'public',privateKey:'private'}});
 await app.locals.pump();assert.equal(sent,1);assert.equal(store.db.prepare('SELECT subscription FROM devices').get().subscription,null);
 store.db.prepare('UPDATE devices SET subscription=? WHERE id=?').run('{}',id);store.enqueue({id:task.taskID});store.resolve(task.taskID,'Approve','mac');await app.locals.pump();assert.equal(sent,1);assert.equal(store.db.prepare('SELECT COUNT(*) AS n FROM outbox').get().n,0);store.close();
});
