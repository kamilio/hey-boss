import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const source=resolve('src/issues/web');
const copy={name:'example',scope:'global',agent:'codex',digest:'original',description:'Example skill',text:'---\nname: example\ndescription: Example\n---\n# Example\nUse /tmp/example for files.\n',path:'/example/.codex/skills/example',word_count:14};
const inventory={ok:true,revision:1,busy:false,selected:['example'],choices:{},max_words:400,message:'Inventory refreshed',machines:[{host:'local',state:'synced',copies:[copy]}]};
const posts=[];
const server=createServer(async(req,res)=>{
 res.setHeader('Content-Type','application/json');
 if(req.url==='/api/bootstrap')return res.end(JSON.stringify({ok:true,csrf:'test',actor:{id:'test'},project:{id:'named:test',name:'Test'},projects:[]}));
 if(req.url==='/api/skills'){if(req.method==='POST'){let s='';for await(const c of req)s+=c;posts.push(JSON.parse(s));}return res.end(JSON.stringify(inventory));}
 const name=req.url.split('?')[0].slice(1)||'skills';
 if(!/^[\w.-]+$/.test(name)){res.statusCode=404;return res.end();}
 try{let bytes=readFileSync(resolve(source,name==='skills'?'skills.html':name));if(name==='skills')bytes=Buffer.from(bytes.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace(/<!--[^]*?-->/g,'')));res.setHeader('Content-Type',name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html');res.end(bytes);}catch{res.statusCode=404;res.end();}
});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
let browser;
try{
 browser=await chromium.launch({channel:'chrome',headless:true});
 const page=await browser.newPage({viewport:{width:1440,height:1000}});const errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('dialog',d=>d.accept());
 await page.goto(`http://127.0.0.1:${server.address().port}/skills`);
 await page.getByRole('button',{name:'Edit',exact:true}).click();
 await page.waitForFunction(()=>!document.querySelector('[data-open-native-editor]').disabled);
 assert.equal(posts.at(-1).action,'open_editor');
 await page.locator('[data-toggle-editor]').first().click();
 const editor=page.locator('#skill-codemirror-host .cm-content');await editor.click();await page.keyboard.press('End');await page.keyboard.press('/');
 assert.equal(await page.locator('#skills-search').evaluate(e=>e===document.activeElement),false,'Slash must stay in the shadow-root Markdown editor');
 assert.match(await editor.textContent(),/\/$/);
 for(const [name,width,dark] of [['desktop',1440,false],['dark',1440,true],['mobile',390,false]]){await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:dark?'dark':'light'});await page.locator('.skill-markdown-editor-card').scrollIntoViewIfNeeded();await page.evaluate(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))));if(process.env.SKILLS_SCREENSHOTS)await page.screenshot({path:resolve(process.env.SKILLS_SCREENSHOTS,`skill-edit-${name}.png`),fullPage:false});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);}
 await page.locator('[data-save-markdown]').click();
 await page.waitForFunction(()=>!document.querySelector('[data-open-native-editor]').disabled);
 assert.equal(posts.at(-1).distribute,true);
 await page.locator('#skills-main').click({position:{x:3,y:3}});await page.keyboard.press('/');
 assert.equal(await page.locator('#skills-search').evaluate(e=>e===document.activeElement),true,'Slash still opens search outside inputs');
 assert.deepEqual(errors,[]);console.log('PASS slash input, desktop/dark/mobile layout and console');
}finally{await browser?.close();await new Promise(r=>server.close(r));}
