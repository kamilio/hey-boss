// Shared desktop/paired assets, synthetic read-only APIs, and isolated Chrome.
import {createServer} from 'node:http';
import {readFileSync, mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const {chromium} = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const source = resolve('src/issues/web');
const projects = [{id:'named:Atlas',name:'Atlas'},{id:'named:Beacon',name:'Beacon'}];
const run = {id:'run-1',project_id:projects[0].id,project_name:'Atlas',number:42,title:'Keep task conversations connected',state:'completed',started_at:Date.now()-60000,finished_at:Date.now()-1000};
const fleet = () => ({ok:true,machines:[{host:'devbox',hostname:'Devbox',state:'connected',heartbeat:Date.now()/1000,workers:[{id:'worker-1',pid:123,intent:'running',managed:true,config:{name:'Atlas worker',projects:[projects[0].id],concurrency:3,enabled:true},runs:[run,{...run,id:'live-run',state:'running',finished_at:null}]}]}],signals:[],conflicts:[]});
const server = createServer((req,res) => {
  const path = new URL(req.url,'http://fixture').pathname;
  res.setHeader('Content-Type','application/json');
  const json = value => res.end(JSON.stringify(value));
  if(['/api/bootstrap','/api/agent-bootstrap'].includes(path)) return json({ok:true,csrf:'fixture',project:projects[0],projects});
  if(path==='/api/fleet/status') return json(fleet());
  if(path==='/api/fleet/conversation') return json({ok:true,messages:[],availability:'waiting'});
  if(path==='/api/action') return json({ok:true,pull_requests:[],next_offset:null});
  try {
    const paired=new URL(req.url,'http://fixture').searchParams.has('paired');
    const assets=path.startsWith('/artifact-web/')?'mobile/public/artifact-web':path.startsWith('/agent-web/')?'mobile/public/agent-web':source;
    const name=paired?'fleet.html':['/agents','/workers','/agents/session'].includes(path)?'fleet.html':path==='/merged-prs'?'merged-prs.html':path.split('/').pop();
    if(!/^[\w.-]+$/.test(name)) throw Error('Not found');
    let body=readFileSync(resolve(paired?'mobile/public/agent-web':assets,name));
    if(name.endsWith('.html')&&!paired) body=body.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace('<!--workers-current-->',name==='fleet.html'?' aria-current="page"':'').replace(/<!--[^]*?-->/g,''));
    if(name==='routes.js') body=body.toString().replace('/* ROUTE_DEFINITIONS */ []',readFileSync(resolve(source,'routes.json'),'utf8'));
    res.setHeader('Content-Type',name.endsWith('.css')?'text/css':name.endsWith('.js')?'text/javascript':name.endsWith('.png')?'image/png':'text/html');
    res.end(body);
  } catch {res.writeHead(404);res.end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
const origin=`http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({hasTouch:true}),errors=[];
  page.setDefaultTimeout(10000);
  page.on('pageerror',error=>errors.push(error.message));
  const visit=async path=>{await page.goto(origin+path);await page.waitForFunction(()=>document.querySelector('#project-name').textContent!=='Projects');};
  await visit('/merged-prs#project=named%3ABeacon&host=devbox');
  assert.equal(await page.locator('.app-navigation #nav-workers').count(),0,'Workers is not project navigation');
  assert.equal(await page.locator('#navigation-more-links #nav-workers').count(),1,'Workers lives in the global overflow');
  const trigger=page.locator('#navigation-more'),workers=page.locator('#nav-workers');
  assert.equal(await workers.getAttribute('href'),'/workers','Workers never inherits project or host scope');
  for(const [width,height] of [[1440,1000],[768,1000],[390,844],[320,760]]) for(const colorScheme of ['light','dark']) {
    await page.setViewportSize({width,height});await page.emulateMedia({colorScheme});
    await trigger.tap();assert.equal(await workers.isVisible(),true);
    const box=await workers.boundingBox();
    assert.ok(box.x>=0&&box.x+box.width<=width&&box.y+box.height<=height&&box.height>=44,'Menu fits and has touch targets');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No page overflow');
    if(process.env.WORKERS_SCREENSHOTS){mkdirSync(process.env.WORKERS_SCREENSHOTS,{recursive:true});await page.screenshot({path:resolve(process.env.WORKERS_SCREENSHOTS,`menu-${width}-${colorScheme}.png`)});}
    await page.keyboard.press('Escape');assert.equal(await workers.isVisible(),false);
  }
  await trigger.focus();await page.keyboard.press('Enter');await page.keyboard.press('Tab');
  assert.equal(await workers.evaluate(el=>el===document.activeElement),true,'Keyboard reaches Workers first');
  await page.keyboard.press('Enter');await page.locator('#worker-board .activity-machine').waitFor();
  await page.locator('.worker-scope-heading').click();
  await page.locator('.activity-worker-summary').click();
  if(process.env.WORKERS_SCREENSHOTS)await page.screenshot({path:resolve(process.env.WORKERS_SCREENSHOTS,'workers-phone.png'),fullPage:true});
  await page.locator('.activity-task').click();
  await page.locator('#session-title').filter({hasText:run.title}).waitFor();
  assert.match(await page.locator('#back').getAttribute('href'),/issue=42/,'Clicking a running task opens its conversation with a task return');
  await visit('/workers');
  assert.equal(await page.locator('.fleet-tabs').count(),0,'No standalone conversation browser');
  assert.equal(await page.locator('#worker-status-view').getAttribute('href'),'/workers');
  await page.locator('#worker-config-view').click();await page.locator('#config-editor').waitFor();
  await page.locator('#project-trigger').click();await page.locator('[data-project="named:Beacon"]').click();
  assert.equal(await page.locator('#config-editor').isVisible(),true,'Project selection preserves global worker settings');
  assert.equal(await workers.getAttribute('href'),'/workers');
  await visit('/agents#project=named%3AAtlas&view=conversations');
  await page.locator('#worker-board .activity-machine').waitFor();
  assert.equal(await page.locator('#projects').count(),0,'Legacy browse URL opens Workers');
  for(const paired of [false,true]) for(const width of [1440,390,320]) {
    await page.setViewportSize({width,height:900});
    await visit('/agents/session'+(paired?'?paired=1':'')+'#project=named%3AAtlas&host=devbox&run=run-1');
    await page.locator('#session-title').filter({hasText:run.title}).waitFor();
    const back=await page.locator('#back').getAttribute('href');
    assert.ok(back.includes('issue=42')&&back.includes('project=named%3AAtlas'),'Conversation returns to its source issue');
    assert.equal(new URL(origin+back).pathname,paired?'/project-resource':'/');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Conversation fits');
    assert.equal(await page.locator('#nav-workers').getAttribute('aria-current'),null,'Conversation does not select global Workers');
    if(process.env.WORKERS_SCREENSHOTS) await page.screenshot({path:resolve(process.env.WORKERS_SCREENSHOTS,`session-${paired?'paired':'desktop'}-${width}.png`)});
  }
  await visit('/agents/session#project=named%3ABeacon&issue=77');
  await page.locator('#session-title').filter({hasText:'Conversation unavailable'}).waitFor();
  assert.match(await page.locator('#back').getAttribute('href'),/issue=77/,'Missing session still returns to task');
  assert.deepEqual(errors,[]);
  console.log('PASS: global Workers menu, keyboard/touch, eight menu layouts, settings, legacy URL, desktop/paired task return, missing session');
} finally {await browser?.close();await new Promise(done=>server.close(done));}
