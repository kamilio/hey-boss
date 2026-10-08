// Uses real UI assets with synthetic API responses; never changes installed skills.
// PLAYWRIGHT_MODULE may point to an installed Playwright package.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFileSync, mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
const {chromium} = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const source = resolve('src/issues/web');
const project = {id:'named:reconnect-test',name:'Reconnect test'};
const copy = {name:'AGENTS.md',scope:'global',agent:'codex',digest:'original',description:'Main Codex instructions',text:'# Example\nOriginal content',path:'/example/.codex/AGENTS.md',word_count:4};
const inventory = {ok:true,revision:17,busy:false,selected:['AGENTS.md'],choices:{},max_words:400,message:'Inventory refreshed',machines:['local','devbox','kamils-macbook-pro.local'].map(host=>({host,state:'synced',scanned_at:1700000000,copies:[copy,{...copy,name:'example-skill',digest:'example'}]}))};
let token='initial',actor='test:actor',mode='',bootstraps=0,posts=[],applied=[];
const server=createServer(async(req,res)=>{
  res.setHeader('Content-Type','application/json');
  res.setHeader('Cache-Control','no-store');
  const fail=(status,message)=>{res.statusCode=status;res.end(JSON.stringify({ok:false,error:{code:status===403?'forbidden':'test_error',message}}));};
  if(req.url==='/api/bootstrap'){
    bootstraps++;
    if(mode==='bootstrap-fails')return fail(503,'Unavailable');
    return res.end(JSON.stringify({ok:true,csrf:token,actor:{id:actor},project,projects:[project]}));
  }
  if(req.url==='/api/skills'){
    if(req.method==='POST'){
      let body='';for await(const chunk of req)body+=chunk;
      posts.push({body,token:req.headers['x-hey-boss-csrf']});
      if(mode==='server-error')return fail(503,'Unavailable');
      if(mode==='forbidden'||req.headers['x-hey-boss-csrf']!==token)return fail(403,'Reload the page to reconnect to this server');
      if(mode==='conflict')return fail(409,'Inventory changed. Scan again.');
      applied.push(JSON.parse(body));
    }
    return res.end(JSON.stringify(inventory));
  }
  const name=req.url.split('?')[0].slice(1)||'skills';
  if(!/^[\w.-]+$/.test(name)){res.statusCode=404;return res.end();}
  try{
    let bytes=readFileSync(resolve(source,name==='skills'?'skills.html':name));
    if(name==='skills')bytes=Buffer.from(bytes.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace(/<!--[^]*?-->/g,'')));
    res.setHeader('Content-Type',name.endsWith('.css')?'text/css':name.endsWith('.js')?'text/javascript':name.endsWith('.png')?'image/png':'text/html');
    res.end(bytes);
  }catch{res.statusCode=404;res.end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
let browser;
try{
  browser=await chromium.launch({channel:'chrome',headless:true});
  const context=await browser.newContext({viewport:{width:1440,height:1000}});
  const page=await context.newPage(),errors=[];
  page.on('pageerror',error=>errors.push(error.message));
  page.on('dialog',dialog=>dialog.accept());
  await page.route('**/api/skills',route=>{
    if(mode==='network'&&route.request().method()==='POST'){
      posts.push({body:route.request().postData(),token:route.request().headers()['x-hey-boss-csrf']});
      return route.abort('failed');
    }
    return route.continue();
  });
  const origin=`http://127.0.0.1:${server.address().port}`;
  const ready=()=>page.waitForFunction(()=>!document.querySelector('#skills-scan').disabled);
  const fresh=async()=>{
    mode='';token='initial';actor='test:actor';posts=[];applied=[];bootstraps=0;
    await page.goto(origin+'/skills');await ready();
  };
  const scan=async()=>{await page.locator('#skills-scan').click();await ready();};
  const screenshot=async name=>{
    if(!process.env.SKILLS_SCREENSHOTS)return;
    mkdirSync(process.env.SKILLS_SCREENSHOTS,{recursive:true});
    await page.screenshot({path:resolve(process.env.SKILLS_SCREENSHOTS,name+'.png'),fullPage:true});
  };
  await fresh();
  await page.locator('[data-select="example-skill"]').check();
  await page.locator('#skills-search').fill('AGENTS');
  token='after-restart';
  await scan();
  await screenshot('restarted');
  assert.equal(await page.locator('#skills-error').isVisible(),false,'Restart reconnects without a reload error');
  assert.equal(posts.length,2,'Only the rejected action is retried');
  assert.equal(bootstraps,2,'Fetch a fresh bootstrap once');
  assert.equal(posts[0].body,posts[1].body,'Retry keeps the exact payload and revision');
  assert.equal(applied.length,1,'Apply the action once');
  assert.equal(await page.locator('#skills-search').inputValue(),'AGENTS');
  assert.match(await page.locator('#selection-summary').textContent(),/2 skills selected.*Unsaved changes/);
  assert.equal(await page.locator('.skill-detail-heading h2').textContent(),'AGENTS.md');

  for(const failure of ['forbidden','actor-changed','bootstrap-fails','server-error','network','conflict']){
    await fresh();mode=failure;
    if(['actor-changed','bootstrap-fails','conflict'].includes(failure))token='rotated';
    if(failure==='actor-changed')actor='different:actor';
    await scan();
    assert.equal(await page.locator('#skills-error').isVisible(),true,failure+': show the failure');
    assert.equal(posts.length,failure==='conflict'?2:1,failure+': no unsafe replay');
    assert.equal(applied.length,0);
    assert.equal(await page.locator('.skill-detail-heading h2').textContent(),'AGENTS.md','Keep inventory on screen');
    if(failure==='forbidden')await screenshot('unrecoverable');
    mode='';token='retry-token';actor='test:actor';
    await page.locator('[data-refresh-inventory]').first().click();await ready();
    assert.equal(await page.locator('#skills-error').isVisible(),false,failure+': explicit retry recovers');
    assert.equal(await page.locator('.skill-rollout-feedback.has-error').count(),0,'Clear both error surfaces');
  }
  await fresh();token='second-token';mode='forbidden';await scan();
  assert.equal(posts.length,2,'Persistent rejection stops after one replay');
  assert.equal(bootstraps,2,'Persistent rejection cannot loop');

  // A successful inventory poll must not imply a rejected mutation succeeded.
  await page.waitForResponse(r=>r.url().endsWith('/api/skills')&&r.request().method()==='GET');
  assert.equal(await page.locator('#skills-error').isVisible(),true);
  await fresh();
  await page.locator('[data-toggle-editor]').click();
  await page.locator('.cm-content').waitFor();
  await page.locator('.cm-content').fill('Unsaved Markdown draft');
  token='editor-restart';await scan();
  assert.equal(await page.locator('.cm-content').innerText(),'Unsaved Markdown draft','Scanning after restart preserves the editor');
  await page.locator('[data-save-markdown="local"]').click();await ready();
  assert.equal(applied.at(-1).content,'Unsaved Markdown draft');
  assert.equal(applied.at(-1).base_digest,'original');
  await fresh();token='save-restart';
  await page.locator('[data-toggle-editor]').click();await page.locator('.cm-content').waitFor();
  await page.locator('.cm-content').fill('Saved after reconnect');
  await page.locator('[data-save-markdown="local"]').click();await ready();
  assert.equal(posts.length,2);assert.equal(posts[0].body,posts[1].body);
  assert.equal(applied.length,1);assert.equal(applied[0].content,'Saved after reconnect');

  for(const width of [1440,768,390,320])for(const colorScheme of ['light','dark']){
    await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${width} ${colorScheme}: no horizontal overflow`);
    await screenshot(`${width}-${colorScheme}`);
  }
  assert.deepEqual(errors,[],'No browser runtime errors');
  console.log('Skills reconnect: restart, bounded replay, identity, failures, draft preservation, save, and responsive themes passed');
}finally{
  if(browser)await browser.close();
  await new Promise(done=>server.close(done));
}
