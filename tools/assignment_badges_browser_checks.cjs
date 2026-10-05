// Run with Playwright available on NODE_PATH. Uses synthetic data only.
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {createServer} = require('node:http');
const {resolve} = require('node:path');
const {chromium, webkit} = require('playwright');
const root = resolve('src/issues/web');
const project = {id:'named:Badge QA',name:'Badge QA',open:4,closed:0,ready:0,blocked:0,deleted:0,unassigned:0,prs_enabled:true};
const issues = [
  {title:'Recover the native stream after upstream closes',assignment:{kind:'agent',actor:'codex:astra',machine_name:'Devbox'},assignee:'codex:astra'},
  {title:'Expose the existing filesystem to packaged Workers',assignment:{kind:'boss'},assignee:'human:boss'},
  {title:'Check required reviews and CI for the release',assignment:{kind:'github',waiting:true,actor:'codex:astra'},assignee:'watcher:github'},
  {title:'Verify the new snapshot on the development machine',assignment:{kind:'machine',machine:'box',machine_name:'Devbox'}},
].map((i,index)=>({number:index+1,state:'open',version:1,labels:['needs-boss'],created_at:Date.now()-86400000,updated_at:Date.now(),created_by:'human:boss',comment_count:2,body:'Fixture issue',comments:[],events:[],pull_requests:[],...i}));
const common={ok:true,csrf:'fixture',actor:{id:'human:boss'},boss:{id:'human:boss',name:'Boss'},project,projects:[project],labels:['needs-boss'],actor_models:{'codex:astra':'gpt-6-astra'},assignees:['codex:astra','human:boss','watcher:github'],assignment_machines:[{id:'box',name:'Devbox'}]};
const requests=[];
const server=createServer(async(req,res)=>{
  if(req.url.startsWith('/api/')){
    let body='';for await(const chunk of req)body+=chunk;
    const data=body?JSON.parse(body):{}, op=data.operation||{};
    requests.push(op);
    let result={...common,tasks:[],notices:[],attachments:[],unread:0};
    if(op.action==='view') result={...result,issue:issues.find(i=>i.number===op.number),comments:[],events:[]};
    else if(op.action==='list') result={...result,issues:issues.filter(i=>(!op.assignee||i.assignee===op.assignee||'machine:'+i.assignment.machine===op.assignee)&&(!op.label||i.labels.includes(op.label))),order_version:1};
    res.setHeader('Content-Type','application/json');res.end(JSON.stringify(result));return;
  }
  try {
    const name=req.url==='/'?'index.html':decodeURIComponent(req.url.split('?')[0]).slice(1);
    let text=readFileSync(resolve(root,name));
    if(name.endsWith('.html')){
      let shell=readFileSync(root+'/app-shell.html','utf8');
      for(const [marker,file] of [['header-actions','issue-header-actions'],['profile','issue-profile'],['project-action','issue-project-action']]) shell=shell.replace('<!--'+marker+'-->',readFileSync(root+'/'+file+'.html','utf8'));
      text=String(text).replace('<!--app-shell-->',shell);
    }
    if(name==='routes.js') text=String(text).replace('/* ROUTE_DEFINITIONS */ []',readFileSync(root+'/routes.json','utf8'));
    res.setHeader('Content-Type',name.endsWith('.js')?'application/javascript':name.endsWith('.css')?'text/css':name.endsWith('.png')?'image/png':'text/html');res.end(text);
  }catch{res.statusCode=404;res.end();}
});
(async()=>{
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  const base='http://127.0.0.1:'+server.address().port+'/#project=named%3ABadge+QA';
  try {
    for(const engine of [chromium,webkit]){
      const browser=await engine.launch({headless:true});
      try {
        for(const touch of [false,true]){
          const context=await browser.newContext({viewport:touch?{width:390,height:844}:{width:1440,height:1000},hasTouch:touch,isMobile:touch});
          const page=await context.newPage(), errors=[]; page.setDefaultTimeout(5000);
          page.on('pageerror',e=>errors.push(e.stack));
          const list=async()=>{await page.mouse.move(0,0);await page.goto(base);await page.locator('.assignment-badge').first().waitFor();};
          const badge=n=>page.locator(`[data-issue-number="${n}"] .assignment-badge`);
          const card=n=>page.locator('#assignment-card-'+n);
          await list();
          for(const [number,owner] of [[1,'codex:astra'],[2,'human:boss'],[3,'watcher:github'],[4,'machine:box']]){
            await list();
            if(touch){
              const box=await badge(number).boundingBox();assert(box.width>=44&&box.height>=44);
              await badge(number).tap();await card(number).waitFor({state:'visible'});
              assert.equal(await page.locator('#owner-filter').inputValue(),'all');
              await card(number).getByRole('link',{name:/Filter by/}).tap();
            }else{
              await badge(number).hover();await card(number).waitFor({state:'visible'});
              assert.equal(await badge(number).innerText(),'');
              await badge(number).click();
            }
            await page.waitForFunction(owner=>model.route.owner===owner&&model.issues.length>0,owner);
            assert.equal(await page.locator('#owner-filter').inputValue(),owner);
            assert(requests.some(op=>op.action==='list'&&op.assignee===owner));
            assert(!new URL(page.url()).hash.includes('issue='));
          }
          await list();
          if(touch)await badge(1).tap();else await badge(1).hover();
          await card(1).waitFor({state:'visible'});
          assert.match(await card(1).innerText(),/Codex · gpt-6-astra/);
          assert.match(await card(1).innerText(),/Devbox/);
          const link=card(1).getByRole('link',{name:'Open agent conversation'});
          assert.match(await link.getAttribute('href'),/agent=codex%3Aastra/);
          if(!touch){
            await link.hover();await page.waitForTimeout(250);assert(await card(1).isVisible());
            await page.mouse.move(0,0);await card(1).waitFor({state:'hidden'});
            await badge(1).focus();await page.keyboard.press('ArrowDown');
            assert(await card(1).getByRole('link',{name:/Filter by/}).evaluate(el=>el===document.activeElement));
            await page.keyboard.press('Escape');assert(!(await card(1).isVisible()));
            assert(await badge(1).evaluate(el=>el===document.activeElement));
          }
          await page.locator('h1').click();await card(1).waitFor({state:'hidden'});
          if(touch)await badge(3).tap();else await badge(3).hover();
          await card(3).getByRole('button',{name:'Open GitHub watcher'}).click();
          await page.locator('.github-watcher-dialog').waitFor({state:'visible'});
          await page.keyboard.press('Escape');
          await list();
          await page.locator('[data-issue-number="1"] .list-label-filter').click();
          await page.waitForFunction(()=>model.route.label==='needs-boss');
          assert.equal(await page.locator('#label-filter').inputValue(),'needs-boss');
          await page.locator('[data-issue-number="1"] .issue-title').click();
          const detailTag=page.locator('.side-labels .list-label-filter');await detailTag.waitFor().catch(async e=>{throw Error(e.message+"\n"+await page.locator("#detail-view").innerText())});await detailTag.click();
          await page.waitForFunction(()=>!model.route.issue&&model.route.label==='needs-boss');
          assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
          await list();
          for(const scheme of ['light','dark']){
            await page.emulateMedia({colorScheme:scheme});
            if(touch)await badge(1).tap();else await badge(1).hover();
            const box=await card(1).boundingBox();assert(box&&box.x>=0&&box.y>=0&&box.x+box.width<=(touch?390:1440));
            if(process.env.BADGE_SCREENSHOTS)await page.screenshot({path:`${process.env.BADGE_SCREENSHOTS}/${engine.name()}-${touch?'touch':'desktop'}-${scheme}.png`});
          }
          assert.deepEqual(errors,[]);
          console.log(engine.name(),touch?'touch':'desktop','passed');
          await context.close();
        }
      }finally{await browser.close();}
    }
  }finally{server.closeAllConnections();await new Promise(r=>server.close(r));}
})().catch(e=>{console.error(e);process.exitCode=1;});
