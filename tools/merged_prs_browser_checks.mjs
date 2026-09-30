// Run with PLAYWRIGHT_MODULE pointing to the installed playwright package.
// Uses the real source assets and synthetic API responses; closes all resources.
import {createServer} from 'node:http';
import {readFileSync, mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const {chromium} = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const source = resolve('src/issues/web');
const project = 'github.com/example/runtime';
const now = Date.now();
const rows = Array.from({length:104}, (_, i) => ({url:`https://github.com/example/runtime/pull/${1000+i}`,title:i===0?'Keep background requests responsive when a companion reconnects and a very long repository name crosses the phone screen':'Improve the runtime '+i,merged_at:now-i*3600000,issues:[{number:i+1,title:i===0?'A linked issue with a long title that should wrap naturally':'Runtime task '+i}]}));
rows.push({url:'https://github.com/example/runtime/pull/999',title:'Earlier merge without a recorded date',merged_at:null,observed_at:now,issues:[]});
let failing=false, delayed=false;
const server=createServer(async(req,res)=>{
  res.setHeader('Content-Type','application/json');
  if(req.url==='/api/bootstrap')return res.end(JSON.stringify({ok:true,csrf:'fixture',project:{id:project,name:'Runtime'},projects:[{id:project,name:'Runtime'},{id:'named:empty',name:'Empty project'}]}));
  if(req.url==='/api/action'){
    let body='';for await(const part of req)body+=part;
    const value=JSON.parse(body);
    if(value.operation.action!=='merged_pull_requests')return res.end(JSON.stringify({ok:true,issues:[]}));
    if(failing){res.statusCode=503;return res.end(JSON.stringify({ok:false,error:{message:'Connection interrupted. Retry.'}}));}
    if(delayed)await new Promise(done=>setTimeout(done,350));
    const data=value.project===project?rows:[],offset=value.operation.offset||0,limit=value.operation.limit;
    return res.end(JSON.stringify({ok:true,pull_requests:data.slice(offset,offset+limit),next_offset:offset+limit<data.length?offset+limit:null}));
  }
  if(req.url==='/mobile'){
    res.setHeader('Content-Type','text/html');return res.end(readFileSync('mobile/public/issue-web/merged-prs.html'));
  }
  if(/^\/issue-web\/[\w.-]+$/.test(req.url)){
    const name=req.url.split('/').pop();
    res.setHeader('Content-Type',name.endsWith('.css')?'text/css':name.endsWith('.js')?'text/javascript':'image/png');
    return res.end(readFileSync(resolve('mobile/public/issue-web',name)));
  }
  const name=req.url.split('?')[0].slice(1)||'merged-prs';
  if(!/^[\w.-]+$/.test(name)){res.statusCode=404;return res.end();}
  try{
    let bytes=readFileSync(resolve(source,name==='merged-prs'?'merged-prs.html':name));
    if(name==='merged-prs')bytes=Buffer.from(bytes.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace(/<!--[^]*?-->/g,'')));
    const type=name.endsWith('.css')?'text/css':name.endsWith('.js')?'text/javascript':name.endsWith('.png')?'image/png':'text/html';
    res.setHeader('Content-Type',type);res.end(bytes);
  }catch{res.statusCode=404;res.end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
const origin=`http://127.0.0.1:${server.address().port}`;
let browser;
try{
  browser=await chromium.launch({channel:'chrome',headless:true});
  const context=await browser.newContext({timezoneId:'America/Chicago'}),page=await context.newPage(),errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  await page.goto(`${origin}/merged-prs#project=${encodeURIComponent(project)}&host=connected-mac`);
  await page.waitForFunction(()=>document.querySelectorAll('.merge-row').length===100);
  assert.equal(await page.locator('#nav-merged-prs').getAttribute('aria-current'),'page');
  assert.equal(await page.locator('#quick-issue-open svg').count(),1,'Shared icons are visible');
  assert.equal(await page.locator('.merge-mark svg').count(),100,'Merge icons are visible');
  assert.match(await page.locator('.merge-meta a').first().getAttribute('href'),/host=connected-mac/);
  assert.match(await page.locator('#nav-issues').getAttribute('href'),/host=connected-mac/,'Navigation preserves the selected machine');
  const output=process.env.MERGED_PR_SCREENSHOTS;
  if(output)mkdirSync(output,{recursive:true});
  for(const [width,height] of [[1440,1000],[768,1000],[390,844],[320,760]])for(const colorScheme of ['light','dark']){
    await page.setViewportSize({width,height});await page.emulateMedia({colorScheme});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${width} ${colorScheme}: no horizontal overflow`);
    assert.ok(await page.locator('.merge-row').first().evaluate(el=>el.getBoundingClientRect().right<=innerWidth),`${width}: cards fit`);
    if(output)await page.screenshot({path:resolve(output,`${width}-${colorScheme}.png`)});
  }
  await page.locator('#merged-more').click();await page.waitForFunction(()=>document.querySelectorAll('.merge-row').length===105);
  assert.equal(await page.locator('#merged-more').isVisible(),false);
  assert.match(await page.locator('.merge-day').last().textContent(),/Merge date unavailable/);
  assert.equal(await page.locator('.merge-title').count(),105);
  failing=true;await page.locator('#merged-refresh').click();await page.locator('#merged-error').waitFor({state:'visible'});
  assert.equal(await page.locator('.merge-row').count(),105,'Refresh failure retains history');
  failing=false;await page.locator('#merged-refresh').click();await page.waitForFunction(()=>document.querySelectorAll('.merge-row').length===100);
  assert.equal(await page.locator('#merged-error').isVisible(),false);
  await page.locator('#project-trigger').click();await page.locator('[data-project="named:empty"]').click();
  await page.waitForFunction(()=>document.querySelector('#merged-status').textContent.includes('No merged PRs'));
  assert.equal(await page.locator('.merge-row').count(),0);
  delayed=true;
  await page.evaluate(project=>location.hash=new URLSearchParams({project}),project);
  await page.waitForTimeout(75);
  await page.evaluate(()=>location.hash='project=named%3Aempty');
  await page.waitForTimeout(500);
  assert.equal(await page.locator('.merge-row').count(),0,'Stale project response cannot replace the current project');
  await page.locator('#merged-refresh').focus();await page.keyboard.press('Enter');
  await page.waitForFunction(()=>!document.querySelector('#merged-refresh').disabled);
  delayed=false;
  await page.goto(`${origin}/mobile#project=${encodeURIComponent(project)}&host=ignored-mobile-host`);
  await page.waitForFunction(()=>document.querySelectorAll('.merge-row').length===100);
  assert.doesNotMatch(await page.locator('.merge-meta a').first().getAttribute('href'),/host=/,'Mobile links stay on the supervisor');
  assert.equal(await page.locator('.project-resources #nav-merged-prs').count(),1);
  for(const width of [390,320])for(const colorScheme of ['light','dark']){
    await page.setViewportSize({width,height:844});await page.emulateMedia({colorScheme});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Paired mobile ${width} ${colorScheme}: no overflow`);
    if(output)await page.screenshot({path:resolve(output,`paired-${width}-${colorScheme}.png`)});
  }
  assert.deepEqual(errors,[]);
  console.log('Passed: day groups, pagination, desktop/tablet/phone light and dark layouts, links, project switching, stale responses, keyboard refresh, empty state, error recovery.');
}finally{await browser?.close();await new Promise(done=>server.close(done));}
