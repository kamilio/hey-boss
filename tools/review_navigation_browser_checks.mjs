// Real shared shell and page assets with read-only synthetic API responses.
// PLAYWRIGHT_MODULE selects an installed Playwright; screenshots are optional.
import {createServer} from 'node:http';
import {readFileSync, mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const {chromium} = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const source = resolve('src/issues/web');
const project = {id:'named:Navigation',name:'Navigation'};
const server = createServer((req,res) => {
  const path = new URL(req.url,'http://fixture').pathname;
  res.setHeader('Content-Type','application/json');
  if(path === '/api/bootstrap') return res.end(JSON.stringify({ok:true,csrf:'fixture',project,projects:[project]}));
  if(path === '/api/action') return res.end(JSON.stringify({ok:true,pull_requests:[],next_offset:null}));
  try {
    const mobile = path.startsWith('/issue-web/');
    const name = path === '/paired' ? 'merged-prs.html' : path === '/merged-prs' ? 'merged-prs.html' : path.slice(1).replace(/^issue-web\//,'');
    if(!/^[\w.-]+$/.test(name)) throw Error('Not found');
    let body = readFileSync(resolve(mobile || path === '/paired' ? 'mobile/public/issue-web' : source,name));
    if(path === '/merged-prs') body = body.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace(/<!--[^]*?-->/g,''));
    res.setHeader('Content-Type',name.endsWith('.css')?'text/css':name.endsWith('.js')?'text/javascript':name.endsWith('.png')?'image/png':'text/html');
    res.end(body);
  } catch {res.writeHead(404);res.end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch({channel:'chrome',headless:true});
  const context = await browser.newContext({hasTouch:true});
  const page = await context.newPage(), errors = [];
  page.on('pageerror',error=>errors.push(error.message));
  await page.goto(origin+'/merged-prs#project=named%3ANavigation&host=connected-mac');
  await page.locator('#merged-status').filter({hasText:'No merged'}).waitFor();
  const trigger = page.getByRole('button',{name:'More navigation',exact:true});
  const review = page.locator('#nav-admin');
  assert.equal(await trigger.count(),1,'Review has an overflow entry point');
  assert.equal(await review.isVisible(),false,'Review is hidden initially');
  assert.equal(await page.locator('.app-navigation #nav-admin').count(),0,'Review is outside the primary tabs');
  for(const [width,height] of [[1440,1000],[768,1000],[390,844],[320,760]]) for(const colorScheme of ['light','dark']) {
    await page.setViewportSize({width,height});
    await page.emulateMedia({colorScheme});
    await trigger.tap();
    assert.equal(await review.isVisible(),true,'Touch opens Review');
    assert.match(await review.getAttribute('href'),/project=named%3ANavigation&host=connected-mac/,'Project and host survive');
    const box = await review.boundingBox();
    assert.ok(box.x>=0 && box.x+box.width<=width && box.y>=0 && box.y+box.height<=height,'Menu fits the viewport');
    assert.ok(box.height>=44,'Touch target is at least 44px');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No horizontal overflow');
    if(process.env.REVIEW_SCREENSHOTS) {
      mkdirSync(process.env.REVIEW_SCREENSHOTS,{recursive:true});
      await page.screenshot({path:resolve(process.env.REVIEW_SCREENSHOTS,`${width}-${colorScheme}.png`)});
    }
    await page.locator('h1').click();
    assert.equal(await review.isVisible(),false,'Outside click dismisses');
  }
  await trigger.focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('Tab');
  assert.equal(await review.evaluate(el=>el===document.activeElement),true,'Tab reaches Review');
  await page.keyboard.press('Escape');
  assert.equal(await review.isVisible(),false,'Escape dismisses');
  assert.equal(await trigger.evaluate(el=>el===document.activeElement),true,'Escape restores trigger focus');
  await page.keyboard.press('Space');
  assert.equal(await review.isVisible(),true,'Space reopens');
  await trigger.click();
  assert.equal(await review.isVisible(),false,'Trigger toggles closed');
  await trigger.click();
  await page.setViewportSize({width:768,height:1000});
  await page.waitForFunction(()=>!document.querySelector('#navigation-more-links').matches(':popover-open'));
  assert.equal(await review.isVisible(),false,'Resizing dismisses the anchored menu');
  await review.evaluate(el=>el.setAttribute('aria-current','page'));
  assert.notEqual(await trigger.evaluate(el=>getComputedStyle(el).backgroundColor),'rgba(0, 0, 0, 0)','Review has an active indicator');
  await page.evaluate(({actions,profile})=>{
    document.querySelector('#quick-issue-open').insertAdjacentHTML('afterend',actions+profile);
    document.querySelector('#project-name').textContent='A project with a very long name';
    HeyBossUI.icons();
  },{actions:readFileSync(resolve(source,'issue-header-actions.html'),'utf8'),profile:readFileSync(resolve(source,'issue-profile.html'),'utf8')});
  await page.setViewportSize({width:320,height:760});
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Full issue header fits a narrow phone');
  await trigger.click();
  if(process.env.REVIEW_SCREENSHOTS) await page.screenshot({path:resolve(process.env.REVIEW_SCREENSHOTS,'320-full-header.png')});
  await page.goto(origin+'/paired#project=named%3ANavigation');
  assert.equal(await page.locator('#navigation-more').isVisible(),false,'Paired UI has no empty menu for unavailable Review');
  assert.deepEqual(errors,[],'No runtime errors');
  console.log('PASS: Review overflow, touch, keyboard, dismissal, context, 8 visual layouts, paired visibility');
} finally {
  await browser?.close();
  await new Promise(done=>server.close(done));
}
