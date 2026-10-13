import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createApp} from './index.mjs';
import {HubStore} from './store.mjs';

test('unpaired mobile links show the app shell while private APIs stay protected',async t=>{
 const store=new HubStore(),app=createApp({store,hubToken:'navigation-test-credential-0000000000',secure:false,origin:'http://localhost'});
 const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));
 t.after(()=>{app.locals.close();server.close();store.close();});
 const base='http://127.0.0.1:'+server.address().port;
 for(const cookie of ['', 'hb_session=expired']){
  for(const path of ['/issues','/jobs','/merged-prs','/mm','/artifacts','/project-resource','/agents','/agents/session','/workers']){
   const response=await fetch(base+path+'?focus=1',{headers:{Cookie:cookie}});
   assert.equal(response.status,200,path);
   assert.match(response.headers.get('content-type'),/text\/html/);
   assert.match(await response.text(),/id="root"/);
   assert.equal(response.headers.get('cache-control'),'no-store');
  }
 }
 for(const path of ['/api/tasks','/api/bootstrap','/api/fleet/status','/api/artifact-bootstrap','/issue-web/app.js']){
  assert.equal((await fetch(base+path)).status,401,path);
 }
});

test('pairing returns to the original mobile page or legacy issue link',async()=>{
 const {pairedDestination}=await import('../src/paired-navigation.js');
 for(const path of ['/issues','/jobs','/merged-prs','/mm','/artifacts','/project-resource','/agents','/agents/session','/workers']){
  const url=new URL(path+'?focus=1#project=named%3AMobile&issue=7','https://phone.example');
  assert.equal(pairedDestination(url),url.pathname+url.search+url.hash);
 }
 assert.equal(pairedDestination(new URL('https://phone.example/#issues')),'/issues');
 assert.equal(pairedDestination(new URL('https://phone.example/#project=named%3AMobile&issue=7')),'/issues#project=named%3AMobile&issue=7');
 assert.equal(pairedDestination(new URL('https://phone.example/?focus=1#view=issues')),'/issues?focus=1#view=issues');
 for(const path of ['/', '/?task=notice&opened=1','/#view=inbox','/?next=https://other.example','/api/tasks']){
  assert.equal(pairedDestination(new URL(path,'https://phone.example')),null,path);
 }
 assert.equal(pairedDestination(new URL('https://other.example/issues')),'/issues');
});

test('phone navigation has Inbox and Issues, with history inside Inbox',()=>{
 const source=readFileSync(new URL('../src/main.jsx',import.meta.url),'utf8');
 const navigation=source.match(/<nav aria-label="Main navigation">([\s\S]*?)<\/nav>/)?.[1];
 assert.ok(navigation,'Main navigation is present');
 for(const label of ['Inbox','Issues'])assert.ok(navigation.includes(label),label+' remains available');
 assert.doesNotMatch(navigation,/Activity|Agents|artifacts/i);
 assert.match(source,/Notification history/);
 assert.match(source,/href="\/issues"/);
});
