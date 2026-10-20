// PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs node tools/check_html_import.mjs
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {startFixture} from './serve_html_fixture.mjs';
const {chromium,webkit}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
for(const engine of [chromium,webkit]){
 const fixture=await startFixture();let browser;
 try{
  browser=await engine.launch({headless:true,...(engine===chromium?{channel:'chrome'}:{})});
  for(const mobile of [false,true]){
   const context=await browser.newContext({viewport:{width:mobile?390:1280,height:900}}),page=await context.newPage();
   const base=mobile?fixture.mobile:fixture.native,label=engine.name()+'-'+(mobile?'paired':'desktop');
   const errors=[];page.on('pageerror',error=>errors.push(error.message));
   if(mobile){const pair=await(await page.request.get(base+'/fixture-pairing')).json();assert((await page.request.post(base+'/api/pair',{data:{code:pair.code}})).ok());}
   await page.goto(base+'/artifacts#'+new URLSearchParams({project:fixture.project,artifact:fixture.full}));
   const edit=async()=>{
    if(mobile)await page.locator('#artifact-edit').click();
    else{await page.getByLabel('More actions',{exact:true}).click();await page.locator('#artifact-edit-browser').click();}
   };
   await edit();
   const field=page.locator('#artifact-body'),input=page.locator('#artifact-import');
   const choose=(name,text,mimeType='text/html')=>input.setInputFiles({name,mimeType,buffer:Buffer.from(text)});
   for(const name of ['page.HTML','page.htm']){
    const source=(name.endsWith('htm')?'\uFEFF':'')+fixture.fullBody.replace('</body>','\n\n[Literal](missing.csv)\n\n</body>')+'\n<!-- '+label+' '+name+' -->\n';
    await choose(name,source,name.endsWith('htm')?'':'text/html');
    await page.waitForFunction(source=>document.querySelector('#artifact-body')?.value===source,source);
    assert.equal(await page.locator('#artifact-error:visible').count(),0);
    await page.locator('#artifact-preview').click();
    await page.locator('#artifact-edit-preview .html-preview[data-state="ready"]').waitFor();
    await page.waitForFunction(()=>document.querySelector('#artifact-edit-preview iframe')?.clientHeight>2800);
    await page.locator('#artifact-preview').click();assert.equal(await field.inputValue(),source);
    await page.locator('#artifact-editor button[type=submit]').click();
    await page.locator('#artifact-reading .html-preview[data-state="ready"]').waitFor();
    await page.reload();await page.locator('#artifact-reading .html-preview[data-state="ready"]').waitFor();
    await page.getByLabel('More actions',{exact:true}).click();
    const downloading=page.waitForEvent('download');await page.locator('#artifact-export').click();
    assert.equal(readFileSync(await(await downloading).path(),'utf8'),source);
    await edit();
   }
   const original=await field.inputValue();
   await input.setInputFiles({name:'invalid.html',mimeType:'text/html',buffer:Buffer.from([0xff,0xfe,0xff])});
   await page.locator('#artifact-error:visible').waitFor();assert.equal(await field.inputValue(),original);
   await input.setInputFiles([{name:'one.html',mimeType:'text/html',buffer:Buffer.from('<!doctype html><h1>One</h1>')},{name:'two.md',mimeType:'text/markdown',buffer:Buffer.from('# Two')}]);
   await page.getByText('Choose one Markdown or HTML document and its linked files together.',{exact:true}).waitFor();
   assert.equal(await field.inputValue(),original);
   await choose('note.MD','# Markdown still imports\n','text/plain');
   await page.waitForFunction(()=>document.querySelector('#artifact-body')?.value==='# Markdown still imports\n');
   assert.equal(await page.locator('#artifact-error:visible').count(),0);
   const markdown='# Download\n\n[HTML download](guide.html)\n',attachment='<!doctype html><h1>Downloaded HTML</h1>\n';
   await input.setInputFiles([{name:'note.md',mimeType:'text/markdown',buffer:Buffer.from(markdown)},{name:'guide.html',mimeType:'text/html',buffer:Buffer.from(attachment)}]);
   await page.waitForFunction(markdown=>document.querySelector('#artifact-body')?.value===markdown,markdown);
   await page.locator('#artifact-editor button[type=submit]').click();
   const downloading=page.waitForEvent('download');await page.getByRole('link',{name:'HTML download',exact:true}).click();
   assert.equal(readFileSync(await(await downloading).path(),'utf8'),attachment);
   assert.equal(errors.length,0,errors.join('\n'));
   console.log('PASS',label,'HTML/HTM import, exact source, full height, save/reload/export, invalid-file recovery, Markdown compatibility');
   await context.close();
  }
 }finally{await browser?.close();await fixture.close();}
}
