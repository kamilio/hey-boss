// cargo build --bin hey-boss; npm --prefix tools/web-assets run build
// PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs node tools/check_html_render.mjs
import assert from 'node:assert/strict';
import {readFileSync, mkdirSync} from 'node:fs';
import {startFixture} from './serve_html_fixture.mjs';
const {chromium,webkit}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
const check=eval('('+readFileSync(new URL('./html_render_browser_checks.js',import.meta.url),'utf8').replace(/;\s*$/,'')+')');
const snapshots=process.env.HTML_SNAPSHOTS;
if(snapshots)mkdirSync(snapshots,{recursive:true});
for(const engine of [chromium,webkit]){
 const fixture=await startFixture();let browser;
 try{
  browser=await engine.launch({headless:true,...(engine===chromium?{channel:'chrome'}:{})});
  for(const width of [1280,390]){
   const context=await browser.newContext({viewport:{width,height:900}}),page=await context.newPage();
   const base=fixture.native,label=engine.name()+'-'+width;
   const errors=[];page.on('pageerror',error=>errors.push(error.message));
   const artifact=id=>base+'/artifacts#'+new URLSearchParams({project:fixture.project,artifact:id});
   for(const [kind,url] of [['artifact',artifact(fixture.artifact)],['issue',base+'/'+'#'+new URLSearchParams({project:fixture.project,issue:fixture.issue})]]){
    await page.goto(url);await check(page);console.log('PASS',label,kind);
    if(kind==='issue'){
     await page.locator('#comment-body').fill('Issue preview state '+label);
     await page.locator('#comment-submit').click();
     await page.getByText('Issue preview state '+label,{exact:true}).waitFor();
     assert.equal(await page.locator('.issue-description .html-preview').frameLocator('iframe').locator('#counter').innerText(),'1','Issue comments preserve the running preview');
     await page.locator('[data-edit]').click();
     const body=await page.locator('#editor-body').inputValue();
     await page.locator('#editor-body').fill(fixture.fullBody);
     await page.locator('#editor-preview').click();
     await page.locator('#editor-rendered .html-preview[data-state="ready"]').waitFor();
     await page.waitForFunction(()=>document.querySelector('#editor-rendered iframe').clientHeight>2800);
     await page.locator('#editor-write').click();assert.equal(await page.locator('#editor-body').inputValue(),fixture.fullBody);
     await page.locator('#editor-body').fill(body+'\n<!-- HTML editor verified -->\n');
     await page.locator('#editor-submit').click();await page.locator('#editor-dialog').waitFor({state:'hidden'});
     await page.locator('.issue-description .html-preview[data-state="ready"]').waitFor();
    }
   }
   await page.goto(artifact(fixture.artifact));
   await page.locator('.html-preview[data-state="ready"]').waitFor();
   await page.locator('.html-preview').frameLocator('iframe').locator('#counter').click();
   // Comments on text after a preview retain source-based selection context.
   await page.evaluate(()=>{
    const p=[...document.querySelectorAll('#artifact-reading > p')].at(-1),range=document.createRange();range.selectNodeContents(p);
    const selection=getSelection();selection.removeAllRanges();selection.addRange(range);
    document.querySelector('#artifact-reading').dispatchEvent(new Event('pointerup'));
   });
   await page.getByRole('button',{name:'Comment on selection',exact:true}).click();
   await page.locator('#artifact-comment').fill('HTML context '+label);
   await page.locator('#artifact-comment-form button[type=submit]').click();
   await page.getByText('HTML context '+label,{exact:true}).waitFor();
   assert.equal(await page.getByText('Outdated selection · discussion preserved',{exact:true}).count(),0);
   assert.equal(await page.locator('.html-preview').frameLocator('iframe').locator('#counter').innerText(),'1','Commenting preserves the running preview');
   await page.goto(artifact(fixture.full));
   await page.locator('.html-preview[data-state="ready"]').waitFor();
   await page.getByLabel('More actions',{exact:true}).click();await page.locator('#artifact-edit-browser').click();
   const field=page.locator('#artifact-body'),source=await field.inputValue();
   assert(source.startsWith('<!doctype html>'));
   const edited=source+'\n<!-- Saved from '+label+' -->\n';await field.fill(edited);
   await page.locator('#artifact-preview').click();
   await page.locator('#artifact-edit-preview .html-preview[data-state="ready"]').waitFor();
   await page.locator('#artifact-preview').click();assert.equal(await field.inputValue(),edited);
   await page.locator('#artifact-editor button[type=submit]').click();
   await page.locator('#artifact-reading .html-preview[data-state="ready"]').waitFor();
   const figure=page.locator('#artifact-reading .html-preview'),frame=figure.frameLocator('iframe');
   await page.waitForFunction(()=>document.querySelector('#artifact-reading iframe').clientHeight>2800);
   assert(await frame.locator('#end').isVisible());
   await frame.locator('#toggle').click();
   await page.waitForFunction(()=>document.querySelector('#artifact-reading iframe').clientHeight<500);
   if(snapshots)for(const [theme,width] of [['light',1280],['dark',390]]){
    await page.emulateMedia({colorScheme:theme});await page.setViewportSize({width,height:900});
    const height=await frame.locator('body').evaluate(n=>Math.ceil(n.scrollHeight));
    await page.waitForFunction(height=>document.querySelector('#artifact-reading iframe').clientHeight>=height,height);
    await page.screenshot({path:snapshots+'/'+label+'-'+theme+'.png',fullPage:true});
   }
   await frame.locator('#toggle').click();await page.waitForFunction(()=>document.querySelector('#artifact-reading iframe').clientHeight>2800);
   await figure.getByRole('button',{name:'Expand',exact:true}).click();
   assert(await figure.locator('iframe').evaluate(n=>n.clientHeight>2800));
   await page.keyboard.press('Escape');
   await page.getByLabel('More actions',{exact:true}).click();
   const downloading=page.waitForEvent('download');await page.locator('#artifact-export').click();
   const download=await downloading;assert.equal(readFileSync(await download.path(),'utf8'),edited);
   // The frame cannot impersonate the parent or fetch authenticated content.
   const height=await figure.locator('iframe').evaluate(n=>n.clientHeight);
   await page.evaluate(()=>window.postMessage({channel:'hey-boss-html',type:'size',height:123},'*'));
   await page.waitForTimeout(50);assert.equal(await figure.locator('iframe').evaluate(n=>n.clientHeight),height);
   assert.equal(await frame.locator('body').evaluate(async()=>{try{await fetch('/api/bootstrap');return true}catch{return false}}),false);
   await page.waitForFunction(()=>document.querySelector('.html-preview').dataset.state==='error');
   assert(await figure.locator('details').evaluate(n=>n.open));
   assert.equal(errors.length,0,errors.join('\n'));
   console.log('PASS',label,'whole HTML, uncapped height, selection comments, editing, source export and failure fallback');
   await context.close();
  }
 }finally{await browser?.close();await fixture.close();}
}
