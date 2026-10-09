// Run with Playwright available on NODE_PATH. Uses synthetic data only.
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {createServer} = require('node:http');
const {resolve} = require('node:path');
const {chromium, webkit} = require('playwright');
const root = resolve('src/issues/web');
const project = {id:'named:Badge QA',name:'Badge QA',open:7,closed:0,ready:0,blocked:0,deleted:0,unassigned:2,prs_enabled:true};
const issues = [
  {title:'Recover the native stream after upstream closes',assignment:{kind:'agent',actor:'codex:astra',machine_name:'Devbox'},assignee:'codex:astra'},
  {title:'Expose the existing filesystem to packaged Workers',assignment:{kind:'boss'},assignee:'human:boss'},
  {title:'Check required reviews and CI for the release',assignment:{kind:'github',waiting:true},assignee:'watcher:github'},
  {title:'Verify the new snapshot on the development machine',assignment:{kind:'machine',machine:'box',machine_name:'Devbox'}},
  {title:'An unassigned issue keeps comments in the same column',assignment:{kind:'unassigned'}},
  {title:'Astra is resolving the PR conflicts',assignment:{kind:'github',waiting:false,actor:'codex:astra',machine_name:'Devbox'},assignee:'codex:astra'},
  {title:'Merge conflicts released the watcher for pickup',assignment:{kind:'github',waiting:false},assignee:null},
].map((i,index)=>({number:index+1,state:'open',version:1,labels:['needs-boss'],created_at:Date.now()-86400000,updated_at:Date.now(),created_by:'human:boss',comment_count:[26,1,0,888,8][index],body:'Fixture issue',body_html:'<p>Inspect the current owner and PR status.</p>',comments:[],events:[],pull_requests:[],...i}));
for (const number of [3,6,7]) {
  const issue=issues[number-1], url=`https://github.com/example/project/pull/${number}`;
  issue.pull_requests=[{url,status:'open',purpose:'fix'}];
  issue.github_status={monitoring:true,fetches:{[url]:{finished_at:Date.now()-60000}},prs:{[url]:{checked_at:Date.now(),evidence:{repository:'example/project',number,conflicts:number===3?'clean':'conflicting',complete:false,required:[]}}}};
}
const common={ok:true,csrf:'fixture',actor:{id:'human:boss'},boss:{id:'human:boss',name:'Boss'},project,projects:[project],labels:['needs-boss'],actor_models:{'codex:astra':'gpt-6-astra'},assignees:['codex:astra','human:boss','watcher:github'],assignment_machines:[{id:'box',name:'Devbox'}]};
const requests=[];
const server=createServer(async(req,res)=>{
  if(req.url.startsWith('/api/')){
    let body='';for await(const chunk of req)body+=chunk;
    const data=body?JSON.parse(body):{}, op=data.operation||{};
    requests.push(op);
    let result={...common,tasks:[],notices:[],attachments:[],unread:0,comments:[],events:[],entries:[],next_before:null};
    if(op.action==='view') result={...result,issue:issues.find(i=>i.number===op.number),comments:[],events:[]};
    else if(op.action==='list') result={...result,issues:issues.filter(i=>(!op.unassigned||!i.assignee)&&(!op.assignee||i.assignee===op.assignee||'machine:'+i.assignment.machine===op.assignee)&&(!op.label||i.labels.includes(op.label))),order_version:1};
    else if(op.action==='assign') {res.statusCode=409;result={ok:false,error:{message:'Assignment changed; refresh before assigning'}};}
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
        for(const width of [1440,768,390,320]){
          const touch=width<768;
          const context=await browser.newContext({viewport:{width,height:1000},hasTouch:touch,isMobile:touch});
          const page=await context.newPage(), errors=[]; page.setDefaultTimeout(5000);
          page.on('pageerror',e=>errors.push(e.stack));
          const list=async()=>{await page.mouse.move(0,0);await page.goto(base);await page.locator('.assignment-badge').first().waitFor();await page.evaluate(()=>window.scrollTo(0,0));await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));};
          const badge=n=>page.locator(`[data-issue-number="${n}"] .assignment-badge`);
          const card=n=>page.locator('#assignment-card-'+n);
          await list();
          const alignment = await page.locator('.issue-row-end').evaluateAll(ends => ends.map(end => {
            const badge = end.querySelector('.assignment-badge')?.getBoundingClientRect();
            const comments = end.querySelector('.comment-count')?.getBoundingClientRect();
            const row = end.closest('.issue-row').getBoundingClientRect();
            return {badge:badge?.x, badgeRight:badge?.right, comments:comments?.x, commentsRight:comments?.right, rowRight:row.right};
          }));
          const badges = alignment.filter(r => r.badge != null), comments = alignment.filter(r => r.comments != null);
          assert(Math.max(...badges.map(r=>r.badge))-Math.min(...badges.map(r=>r.badge))<1, 'Badge column aligns across empty, short and long counts');
          assert(Math.max(...comments.map(r=>r.comments))-Math.min(...comments.map(r=>r.comments))<1, 'Comments align even without an assignee');
          assert(badges.every(r=>r.commentsRight==null||r.commentsRight<r.badge), 'Badges are to the right of comments');
          assert(badges.every(r=>r.rowRight-r.badgeRight<25), 'Badges sit at the far right');
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
          const showOwner = async number => {
            await badge(number).scrollIntoViewIfNeeded();
            await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
            if(touch)await badge(number).tap();else {await badge(number).focus();await badge(number).press('ArrowDown');}
            await card(number).waitFor({state:'visible'});
          };
          for (const [number,label,selection] of [[6,'Codex · gpt-6-astra','active'],[7,'Unassigned','active']]) {
            await list();
            await showOwner(number);
            assert.equal(await card(number).locator('strong').innerText(),label);
            const filter=card(number).getByRole('link',{name:'Filter by '+label});
            if(touch)await filter.tap();else await filter.press('Enter');
            await page.waitForFunction(owner=>model.route.owner===owner,number===6?'codex:astra':'unassigned');
            await list();
            await showOwner(number);
            await card(number).getByRole('button',{name:'Open GitHub watcher'}).click();
            await page.locator('.github-watcher-dialog').getByText('Merge conflicts',{exact:true}).waitFor();
            await page.keyboard.press('Escape');
            await page.locator(`[data-issue-number="${number}"] .issue-title`).click();
            await page.locator('#issue-assignment').waitFor();
            assert.equal(await page.locator('#issue-assignment').inputValue(),selection);
            assert.equal(await page.locator('#issue-assignment option:checked').innerText(),label);
            assert(await page.getByRole('button',{name:'Fetch now',exact:true}).isVisible());
            if(number===6)assert.match(await page.getByRole('link',{name:'Agent conversation →'}).getAttribute('href'),/agent=codex%3Aastra/);
            for(const scheme of ['light','dark']) {
              await page.emulateMedia({colorScheme:scheme});
              assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
              if(process.env.BADGE_SCREENSHOTS)await page.screenshot({path:`${process.env.BADGE_SCREENSHOTS}/${engine.name()}-${width}-${scheme}-owner-${number}.png`,fullPage:true});
            }
            await page.locator('#issue-assignment').selectOption('github');
            await page.getByText('Assignment changed; refresh before assigning',{exact:true}).waitFor();
            assert.equal(await page.locator('#issue-assignment').inputValue(),selection,'Failed changes restore actual owner');
          }
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
            const colors=await page.locator('.assignment-badge').evaluateAll(badges=>badges.map(b=>getComputedStyle(b).color));
            assert.notEqual(colors[0],colors[1], 'Workers have a distinct green color');
            assert.notEqual(colors[3],colors[1], 'Machines have a distinct orange color');
            assert.notEqual(colors[0],colors[3], 'Machine and worker colors differ');
            if(process.env.BADGE_SCREENSHOTS)await page.screenshot({path:`${process.env.BADGE_SCREENSHOTS}/${engine.name()}-${width}-${scheme}-aligned.png`});
            if(touch)await badge(1).tap();else await badge(1).hover();
            const box=await card(1).boundingBox();assert(box&&box.x>=0&&box.y>=0&&box.x+box.width<=width);
            if(process.env.BADGE_SCREENSHOTS)await page.screenshot({path:`${process.env.BADGE_SCREENSHOTS}/${engine.name()}-${width}-${scheme}.png`});
          }
          assert.deepEqual(errors,[]);
          console.log(engine.name(),width,touch?'touch':'desktop','passed');
          await context.close();
        }
      }finally{await browser.close();}
    }
  }finally{server.closeAllConnections();await new Promise(r=>server.close(r));}
})().catch(e=>{console.error(e);process.exitCode=1;});
