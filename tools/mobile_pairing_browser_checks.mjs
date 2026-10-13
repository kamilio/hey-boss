// Run after `npm --prefix mobile run build`. Uses a disposable, synthetic relay.
// PLAYWRIGHT_MODULE may point to an existing local Playwright installation.
import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {createServer} from 'node:http';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const require=createRequire(import.meta.url);
const {chromium,webkit,devices}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const store=new HubStore();
const server=createServer();server.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));
const base='http://127.0.0.1:'+server.address().port;
const app=createApp({store,hubToken:'synthetic-mobile-pairing-fixture-000000',secure:false,origin:base});server.on('request',app);
let passed=0;
try{
 for(const type of [webkit,chromium]){
  const browser=await type.launch();
  try{
   for(const [start,end,selector] of [
    ['/issues?focus=1#project=named%3AMobile&issue=7',null,'#issue-list'],
    ['/#project=named%3AMobile&issue=7','/issues#project=named%3AMobile&issue=7','#issue-list'],
    ['/jobs#project=named%3AMobile',null,'#jobs-main'],
    ['/artifacts#project=named%3AMobile&artifact=a-test',null,'#artifact-main'],
    ['/?task=missing-notice&opened=1',null,'#page-heading'],
   ]){
    const context=await browser.newContext({...devices['iPhone 15'],serviceWorkers:'block'});
    const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
    try{
     await context.addCookies([{name:'hb_session',value:'expired-session',url:base}]);
     await page.goto(base+start);
     await page.getByRole('heading',{name:'Connect your iPhone'}).waitFor();
     assert.equal(page.url(),base+start,'The original link survives the pairing screen');
     assert.equal((await context.request.get(base+'/api/tasks')).status(),401);
     assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
     await page.getByRole('textbox',{name:'Pairing code'}).fill(store.pairing());
     await page.getByRole('button',{name:'Connect',exact:true}).click();
     await page.locator(selector).waitFor();
     await page.waitForURL(base+(end??start));
     assert.equal((await context.request.get(base+'/api/tasks')).status(),200);
     assert.deepEqual(errors,[]);
     passed++;
    }catch(error){throw new Error(`${type.name()} ${start}: ${error.message}\n${await page.locator('body').innerText()}`,{cause:error});}
    finally{await context.close();}
   }
  }finally{await browser.close();}
 }
 console.log(`Mobile pairing: ${passed} Safari/Chrome link and session recovery checks passed`);
}finally{app.locals.close();await new Promise(r=>server.close(r));store.close();}
