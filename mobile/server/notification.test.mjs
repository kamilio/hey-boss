import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import {HubStore} from './store.mjs';
import {createApp} from './index.mjs';
import {markdownText,preview,pushContent} from './markdown-text.mjs';

test('clear preserves winners and history, cancels requests, and leaves new arrivals pending',()=>{
 const store=new HubStore();
 for(const [id,kind,commentsEnabled] of [['update','update',false],['alert','alert',false],['approval','approval',false],['prompt','prompt',false],['review','update',true],['winner','approval',false],['arrival','alert',false]])store.upsert({taskID:id,kind,commentsEnabled,title:id,question:'Body',description:'Summary',options:['Approve']});
 store.resolve('winner','Approve','mac');const winner=store.get('winner');
 assert.throws(()=>store.clear(['update','missing']),/no longer available/);assert.equal(store.get('update').status,'pending');
 for(const ids of [[],['update','update'],[''],Array(10001).fill('update')])assert.throws(()=>store.clear(ids));
 assert.equal(store.clear(['update','alert','approval','prompt','review','winner']),5);
 for(const id of ['update','alert'])assert.equal(store.get(id).status,'ok');
 for(const id of ['approval','prompt','review']){assert.equal(store.get(id).status,'cancelled');assert.equal(store.get(id).result,null);}
 assert.deepEqual(store.get('winner'),winner);assert.equal(store.get('arrival').status,'pending');
 assert.equal(store.clear(['update','review','winner']),0);assert.equal(store.outcomes().length,6);assert.equal(store.list().length,7);store.close();
});

test('clear endpoint requires authentication, same origin and an explicit snapshot',async t=>{
 const store=new HubStore(),app=createApp({store,hubToken:'x'.repeat(64),origin:'http://127.0.0.1',secure:false});
 const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));t.after(()=>{app.locals.close();server.close();store.close();});
 store.upsert({taskID:'notice',kind:'alert',title:'Ready',question:'Ready',description:'',options:[]});
 const base='http://127.0.0.1:'+server.address().port,headers={Cookie:'hb_session='+store.pair(store.pairing()).secret,'Content-Type':'application/json'};
 const post=(headers,body)=>fetch(base+'/api/tasks/clear',{method:'POST',headers,body:JSON.stringify(body)});
 assert.equal((await post({},{})).status,401);assert.equal((await post({...headers,Origin:'https://evil.invalid'},{taskIDs:['notice']})).status,403);
 assert.equal((await post(headers,{})).status,400);assert.equal(store.get('notice').status,'pending');
 const response=await post(headers,{taskIDs:['notice']});assert.equal(response.status,200);assert.equal((await response.json()).cleared,1);
 store.upsert({taskID:'winner',kind:'approval',title:'Ship?',question:'Ship?',description:'',options:['Approve']});store.resolve('winner','Approve','phone');
 store.upsert({taskID:'bridge-notice',kind:'alert',title:'Ready',question:'Ready',description:'',options:[]});
 const bridgePost=headers=>fetch(base+'/api/bridge/tasks/clear',{method:'POST',headers,body:JSON.stringify({taskIDs:['bridge-notice','winner']})});
 assert.equal((await bridgePost({})).status,401);
 const bridge=await bridgePost({Authorization:'Bearer '+'x'.repeat(64),'Content-Type':'application/json'});assert.equal(bridge.status,200);
 const result=await bridge.json();assert.equal(result.cleared,1);assert.equal(result.tasks[0].handledBy,'mac');assert.equal(result.tasks[1].result,'Approve');
});

test('Inbox summaries include every unread notice beyond the Activity limit',()=>{
 const store=new HubStore();for(let i=0;i<305;i++)store.upsert({taskID:'notice-'+i,kind:'alert',title:'Ready',question:'Ready',description:'',options:[]});
 assert.equal(store.summaries().length,305);assert.equal(store.clear(store.summaries().map(t=>t.taskID)),305);
 assert.equal(store.summaries().length,300);assert.equal(store.list().length,300);store.close();
});

test('push previews preserve readable Markdown structure without raw syntax',()=>{
 assert.equal(markdownText('# Ready\n\n**Migration** is _complete_. [Review PR](https://example.com) and `ship()`.'),'Ready\n\nMigration is complete. Review PR and ship().');
 assert.equal(markdownText('- [x] Build\n- [ ] Review'),'• ✓ Build\n• ☐ Review');
 assert.equal(markdownText('3. One\n4. Two'),'3. One\n4. Two');
 assert.equal(markdownText('> Safe **quote**\n\n```swift\nlet result = true\n```'),'Safe quote\n\nlet result = true');
 assert.equal(markdownText('| Host | Status |\n| --- | --- |\n| Mac | Ready |'),'Host · Status\nMac · Ready');
 assert.equal(markdownText('~~Old~~ ![Diagram](https://example.com/x.png)'),'Old Diagram');
 assert.equal(markdownText('<script>bad()</script>\n\nVisible'),'Visible');
 assert.equal(markdownText('First  \nsecond'),'First\nsecond');
 const emoji='👨‍👩‍👧‍👦';assert.equal(preview(emoji.repeat(10),3),emoji.repeat(2)+'…');
 assert.deepEqual(pushContent({title:'**Ready**',question:'Summary',description:'# Full report'}),{title:'Ready',body:'Summary'});
 assert.deepEqual(pushContent({kind:'update',title:'**Ready**',question:'# Full report\n\nVerbose details',description:'**Brief** summary'}),{title:'Ready',body:'Brief summary'});
});
test('opening updates is idempotent, durable, and never answers questions',()=>{
 const store=new HubStore();const row={taskID:'update',kind:'update',title:'Update',question:'Summary',description:'# Report',options:[]};store.upsert(row);
 const read=store.open('update');assert.equal(read.status,'ok');assert.equal(read.handledBy,'phone');assert.equal(store.outcomes().length,1);
 assert.equal(store.open('update').version,read.version);store.ack('update',read.version);assert.equal(store.outcomes().length,0);
 for(const kind of ['approval','prompt']){store.upsert({...row,taskID:kind,kind,options:['Approve']});assert.equal(store.open(kind).status,'pending');}
 store.resolve('approval','Approve','mac');assert.equal(store.open('approval').result,'Approve');store.close();
});
test('authenticated notification opens preserve full documents with bounded inbox and native replies',async t=>{
 const store=new HubStore(),key='x'.repeat(64),app=createApp({store,hubToken:key,origin:'http://127.0.0.1',secure:false});
 const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));t.after(()=>{app.locals.close();server.close();store.close();});
 const base='http://127.0.0.1:'+server.address().port,secret=store.pair(store.pairing()).secret;
 const phone={Cookie:'hb_session='+secret,'Content-Type':'application/json'},bridge={Authorization:'Bearer '+key,'Content-Type':'application/json'};
 const document='# Report\n\n'+('A **long** document.\n'.repeat(25000));
 const row={taskID:'large',kind:'update',title:'Full update',question:document,description:'**Read** this',options:[]};
 const publish=await fetch(base+'/api/bridge/tasks/large',{method:'PUT',headers:bridge,body:JSON.stringify(row)});assert.equal(publish.status,200);assert.ok((await publish.text()).length<1000);
 const inbox=await (await fetch(base+'/api/tasks',{headers:phone})).json();assert.ok(inbox.tasks[0].question.length<600);assert.equal(inbox.tasks[0].description,'Read this');
 assert.equal((await (await fetch(base+'/api/tasks/large',{headers:phone})).json()).task.question,document);
 assert.equal((await fetch(base+'/api/tasks/large/open',{method:'POST',body:'{}'})).status,401);
 assert.equal((await fetch(base+'/api/tasks/large/open',{method:'POST',headers:{...phone,Origin:'https://evil.invalid'},body:'{}'})).status,403);
 for(let i=0;i<2;i++)assert.equal((await fetch(base+'/api/tasks/large/open',{method:'POST',headers:phone,body:'{}'})).status,200);
 const outcome=await (await fetch(base+'/api/bridge/tasks',{headers:bridge})).json();assert.equal(outcome.tasks[0].status,'ok');assert.ok(JSON.stringify(outcome).length<1000);
});
test('notification click saves a read receipt before focusing the correct app',async()=>{
 const handlers={},calls=[];const self={importScripts(){},addEventListener(type,fn){handlers[type]=fn;},location:{origin:'https://example.com'},clients:{async matchAll(){return [{url:'https://example.com',async navigate(url){calls.push(['navigate',url]);},async focus(){calls.push(['focus']);}}];}}};
 vm.runInNewContext(readFileSync(new URL('../public/sw.js',import.meta.url),'utf8'),{self,URL,HeyBossReceipts:{async open(id){calls.push(['open',id]);}}});
 let waiting;handlers.notificationclick({notification:{data:{id:'the-update'},close(){calls.push(['close']);}},waitUntil(promise){waiting=promise;}});await waiting;
 assert.deepEqual(calls.map(x=>x[0]),['close','open','navigate','focus']);assert.equal(calls[1][1],'the-update');assert.match(calls[2][1],/task=the-update&opened=1/);
});
test('stalled receipt delivery times out, preserves queued opens, and stops an offline retry burst',async()=>{
 const saved=new Map();let attempts=0;
 const connection={transaction(){const tx={};tx.objectStore=()=>({put(value){saved.set(value.id,value);queueMicrotask(()=>tx.oncomplete());return {result:undefined};},delete(id){saved.delete(id);queueMicrotask(()=>tx.oncomplete());return {result:undefined};},getAll(){queueMicrotask(()=>tx.oncomplete());return {result:[...saved.values()]};}});return tx;}};
 const context={AbortController,clearTimeout,queueMicrotask,indexedDB:{open(){const request={result:connection};queueMicrotask(()=>request.onsuccess());return request;}},setTimeout:(callback,ms)=>setTimeout(callback,ms===2500?5:ms),fetch:(url,options)=>{attempts++;return new Promise((resolve,reject)=>options.signal.addEventListener('abort',()=>reject(Error('timeout'))));}};
 vm.runInNewContext(readFileSync(new URL('../public/read-receipts.js',import.meta.url),'utf8'),context);
 await assert.rejects(context.HeyBossReceipts.open('first'),/timeout/);await assert.rejects(context.HeyBossReceipts.open('second'),/timeout/);
 assert.equal(saved.size,2);await context.HeyBossReceipts.flush();assert.equal(attempts,3);assert.equal(saved.size,2);
});
test('a slow read receipt does not delay opening the notification reader',async()=>{
 const handlers={};let opened=false,release,finished=false;
 const receipt=new Promise(resolve=>{release=resolve;});
 const self={importScripts(){},addEventListener(type,fn){handlers[type]=fn;},location:{origin:'https://example.com'},clients:{async matchAll(){return [];},async openWindow(url){opened=url.includes('task=update');}}};
 vm.runInNewContext(readFileSync(new URL('../public/sw.js',import.meta.url),'utf8'),{self,URL,HeyBossReceipts:{open(){return receipt;}}});
 let waiting;handlers.notificationclick({notification:{data:{id:'update'},close(){}},waitUntil(promise){waiting=promise.then(()=>{finished=true;});}});
 await new Promise(resolve=>setImmediate(resolve));assert.equal(opened,true);assert.equal(finished,false);release();await waiting;assert.equal(finished,true);
});
