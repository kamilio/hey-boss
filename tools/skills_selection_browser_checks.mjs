// Real UI with delayed fleet responses; never writes installed skills.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFileSync, mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
const source=resolve('src/issues/web');
const copy=(digest,text)=>({name:'AGENTS.md',scope:'global',agent:'codex',digest,text,word_count:4,path:'/fixture/.codex/AGENTS.md'});
const versions=[copy('local-version','# Fixture\nLocal version'),copy('remote-version','# Fixture\nRemote version')];
const initial=()=>({ok:true,revision:17,busy:true,selected:['AGENTS.md'],choices:{'AGENTS.md':'local-version'},max_words:400,message:'Saved AGENTS.md and distributing AGENTS.md…',machines:['local','devbox','kamils-macbook-pro.local'].map((host,i)=>({host,state:'online',scanned_at:1700000000,copies:[versions[i?1:0]]}))});
let inventory=initial(),posts=[],release;
const server=createServer(async(req,res)=>{
  res.setHeader('Cache-Control','no-store');res.setHeader('Content-Type','application/json');
  if(req.url==='/api/bootstrap')return res.end(JSON.stringify({ok:true,csrf:'fixture',actor:{id:'fixture'},projects:[]}));
  if(req.url==='/api/skills'){
    if(req.method==='POST'){
      let body='';for await(const chunk of req)body+=chunk;
      posts.push(JSON.parse(body));
      const reply=structuredClone(inventory);
      reply.choices=posts.at(-1).choices;reply.busy=true;
      reply.message='Distributing selected versions…';
      await new Promise(done=>{release=done;});
      inventory=reply;
    }
    return res.end(JSON.stringify(inventory));
  }
  const name=req.url.split('?')[0].slice(1)||'skills';
  if(!/^[\w.-]+$/.test(name)){res.statusCode=404;return res.end();}
  try{
    let bytes=readFileSync(resolve(source,name==='skills'?'skills.html':name));
    if(name==='skills')bytes=Buffer.from(bytes.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace(/<!--[^]*?-->/g,'')));
    res.setHeader('Content-Type',name.endsWith('.css')?'text/css':name.endsWith('.js')?'text/javascript':name.endsWith('.png')?'image/png':'text/html');res.end(bytes);
  }catch{res.statusCode=404;res.end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
let browser;
try{
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:1100}}),errors=[];
  page.on('pageerror',e=>errors.push(e.message));page.on('dialog',d=>d.accept());
  await page.goto(`http://127.0.0.1:${server.address().port}/skills`);
  await page.locator('.skill-version').first().waitFor();
  const choose=digest=>page.locator(`[data-choose="${digest}"]`);
  const apply=()=>page.locator('[data-unify]');
  assert.equal(await choose('remote-version').count(),1,'Each version remains selectable during distribution');
  assert.equal(await choose('remote-version').isEnabled(),true);
  await choose('remote-version').click();
  assert.equal(await choose('remote-version').getAttribute('aria-pressed'),'true');
  assert.equal(await apply().isDisabled(),true,'Do not overlap fleet writes');
  assert.equal(posts.length,0,'Selecting alone never writes to machines');
  await page.waitForResponse(r=>r.url().endsWith('/api/skills'));
  assert.equal(await choose('remote-version').getAttribute('aria-pressed'),'true','Busy polls preserve the choice');
  const screenshot=async name=>{
    if(!process.env.SKILLS_SCREENSHOTS)return;
    mkdirSync(process.env.SKILLS_SCREENSHOTS,{recursive:true});
    await page.locator('#skills-detail').screenshot({path:resolve(process.env.SKILLS_SCREENSHOTS,name+'.png')});
  };
  for(const width of [1440,768,390,320])for(const colorScheme of ['light','dark']){
    await page.setViewportSize({width,height:1100});await page.emulateMedia({colorScheme});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${width} ${colorScheme}: no overflow`);
    assert.ok(await choose('local-version').isEnabled());
    assert.ok(await page.locator('.gh-diff-file-meta').evaluate(el=>el.getBoundingClientRect().right<=el.closest('.gh-diff-file').getBoundingClientRect().right),`${width} ${colorScheme}: diff actions fit`);
    await screenshot(`busy-${width}-${colorScheme}`);
  }
  inventory.busy=false;inventory.revision++;
  inventory.message='Distribution finished · 1 machine needs attention.';
  inventory.machines[1].error='Machine timed out. Scan again when available';
  await page.waitForFunction(()=>!document.querySelector('[data-unify]').disabled);
  assert.equal(await choose('remote-version').getAttribute('aria-pressed'),'true','Completion preserves the choice');
  await screenshot('retry-ready');
  await apply().click();
  await page.waitForFunction(()=>document.querySelector('[data-unify]').disabled);
  assert.equal(posts.length,1);assert.equal(posts[0].choices['AGENTS.md'],'remote-version');
  assert.equal(posts[0].revision,18,'Apply uses the refreshed inventory revision');
  // Change selection before the old mutation response arrives.
  await choose('local-version').focus();await page.keyboard.press('Enter');
  assert.equal(await choose('local-version').getAttribute('aria-pressed'),'true');
  const response=page.waitForResponse(r=>r.request().method()==='POST');release();await response;
  await page.waitForResponse(r=>r.url().endsWith('/api/skills')&&r.request().method()==='GET');
  assert.equal(await choose('local-version').getAttribute('aria-pressed'),'true','Late mutation responses preserve a newer choice');
  inventory.busy=false;inventory.revision++;
  await page.waitForFunction(()=>!document.querySelector('[data-unify]').disabled);
  assert.equal(posts.length,1,'No automatic queued write');
  await apply().click();await page.waitForFunction(()=>document.querySelector('[data-unify]').disabled);
  assert.equal(posts.length,2);assert.equal(posts[1].choices['AGENTS.md'],'local-version');
  const second=page.waitForResponse(r=>r.request().method()==='POST');release();await second;
  inventory.busy=false;inventory.revision++;
  // The chosen version disappeared; never silently substitute another version.
  inventory.machines.forEach(m=>{m.copies=[versions[1]];});
  await page.waitForFunction(()=>document.querySelectorAll('.skill-version').length===1);
  assert.equal(await apply().isDisabled(),true,'A missing choice requires review');
  await choose('remote-version').click();assert.equal(await apply().isEnabled(),true);
  await screenshot('reviewed');
  // Save & unify has the same late-response hazard, including an open editor.
  inventory=initial();inventory.busy=false;
  await page.reload();await choose('local-version').waitFor();
  await page.locator('[data-toggle-editor]').click();await page.locator('.cm-content').waitFor();
  await page.locator('.cm-content').fill('# Fixture\nEdited local version');
  await page.locator('[data-save-markdown="distribute"]').click();
  await choose('remote-version').click();
  assert.equal(await choose('remote-version').getAttribute('aria-pressed'),'true');
  const saved=page.waitForResponse(r=>r.request().method()==='POST');release();await saved;
  await page.waitForResponse(r=>r.url().endsWith('/api/skills')&&r.request().method()==='GET');
  assert.equal(await choose('remote-version').getAttribute('aria-pressed'),'true','Save & unify preserves a later choice');
  assert.deepEqual(errors,[]);
  console.log('Skill selection: busy, polling, timeout recovery, keyboard, delayed responses, stale versions, responsive light/dark passed');
}finally{
  release?.();if(browser)await browser.close();
  await new Promise(done=>server.close(done));
}
