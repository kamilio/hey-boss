import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createApp} from './index.mjs';
import {HubStore} from './store.mjs';
import {guide} from '../agent-guidance.mjs';

test('every built mobile page shares hidden guidance and authenticated pages stay protected',async t=>{
 const store=new HubStore();
 const app=createApp({store,hubToken:'synthetic-test-key'.repeat(4),secure:false});
 const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));
 t.after(()=>{app.locals.close();server.close();store.close();});
 const base='http://127.0.0.1:'+server.address().port;
 const paired=await fetch(base+'/api/pair',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({code:store.pairing()})});
 const headers={Cookie:paired.headers.get('set-cookie').split(';')[0]};
 for(const path of ['/','/issues','/mm','/artifacts','/project-resource','/agents','/agents/session']){
  if(path!=='/')assert.equal((await fetch(base+path)).status,401,path);
  const response=await fetch(base+path,{headers});assert.equal(response.status,200,path);
  const html=await response.text();
  assert.equal(html.match(/id="hey-boss-agent-guide"/g)?.length,1,path);
  assert.ok(html.includes('<pre hidden'),path);
  assert.ok(html.includes('href="/llms.txt"'),path);
  assert.ok(html.includes('src="/agent-guide.js"'),path);
  assert.ok(html.includes('hey-boss lookup'),path);
 }
 const response=await fetch(base+'/llms.txt');assert.equal(response.status,200);
 assert.equal(await response.text(),guide);
 assert.equal(await(await fetch(base+'/agent-guide.js')).text(),readFileSync(new URL('../../src/issues/web/agent-guide.js',import.meta.url),'utf8'));
});
