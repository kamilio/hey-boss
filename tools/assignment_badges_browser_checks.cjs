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
  {title:'Draft with discussion keeps its actions aligned',draft:true,comment_count:2,assignment:{kind:'unassigned'}},
  {title:'Draft without comments keeps its actions aligned',draft:true,comment_count:0,assignment:{kind:'unassigned'}},
  {title:'Unassigned issue without comments keeps its actions aligned',comment_count:0,assignment:{kind:'unassigned'}},
  {title:'An agent on another machine shows elapsed time',comment_count:0,assignment:{kind:'machine',machine:'box',machine_name:'Devbox'},remote_attempts:[{actor:'codex:astra',machine:'box',host:'Devbox',state:'running'},{actor:'codex:other',machine:'other',host:'Other',state:'running'}]},
  {title:'Past agent work belongs in history',assignment:{kind:'unassigned'},closed_by:'codex:previous'},
].map((i,index)=>({number:index+1,state:'open',version:1,labels:['needs-boss'],created_at:Date.now()-86400000,updated_at:Date.now(),created_by:'human:boss',comment_count:[26,1,0,888,8][index],body:'Fixture issue',body_html:'<p>Inspect the current owner and PR status.</p>',comments:[],events:[],pull_requests:[],...i}));
for (const number of [3,6,7]) {
  const issue=issues[number-1], url=`https://github.com/example/project/pull/${number}`;
  issue.pull_requests=[{url,status:'open',purpose:'fix'}];
  issue.github_status={monitoring:true,fetches:{[url]:{finished_at:Date.now()-60000}},prs:{[url]:{checked_at:Date.now(),evidence:{repository:'example/project',number,conflicts:number===3?'clean':'conflicting',complete:false,required:[]}}}};
}
const common={ok:true,csrf:'fixture',actor:{id:'human:boss'},boss:{id:'human:boss',name:'Boss'},project,projects:[project],labels:['needs-boss'],actor_models:{'codex:astra':'gpt-6-astra'},assignees:['codex:astra','human:boss','watcher:github'],assignment_machines:[{id:'box',name:'Devbox'}]};
const requests=[];
const server=createServer(async(req,res)=>{
  if(req.url.startsWith('/api/fleet/assignment')){
    const query=new URL(req.url,'http://fixture').searchParams;
    res.setHeader('Content-Type','application/json');
    res.end(JSON.stringify({ok:true,online:true,machine:{state:'connected',heartbeat:Date.now()/1000},run:{project_id:project.id,number:Number(query.get('issue')),actor_id:query.get('agent'),state:'running',started_at:Date.now()-125000,finished_at:null}}));return;
  }
  if(req.url.startsWith('/api/')){
    let body='';for await(const chunk of req)body+=chunk;
    const data=body?JSON.parse(body):{}, op=data.operation||{};
    requests.push(op);
    let result={...common,tasks:[],notices:[],attachments:[],unread:0,comments:[],events:[],entries:[],next_before:null};
    if(op.action==='view') result={...result,issue:issues.find(i=>i.number===op.number),comments:[],events:[]};
    else if(op.action==='list') result={...result,issues:issues.filter(i=>(!op.unassigned||!i.assignee)&&(!op.assignee||i.assignee===op.assignee||i.remote_attempts?.some(r=>r.actor===op.assignee)||'machine:'+i.assignment.machine===op.assignee)&&(!op.label||i.labels.includes(op.label))),order_version:1};
    else if(op.action==='assign') {res.statusCode=409;result={ok:false,error:{message:'Assignment changed; refresh before assigning'}};}
    res.setHeader('Content-Type','application/json');res.end(JSON.stringify(result));return;
  }
  try {
    const name=['/','/issues'].includes(req.url)?'index.html':decodeURIComponent(req.url.split('?')[0]).replace(/^\/(?:issue-web\/)?/,'');
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
  const base='http://127.0.0.1:'+server.address().port+'/'+'#project=named%3ABadge+QA';
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
          assert.equal(await badge(12).count(),0,'Past work does not look assigned');
          await page.locator('[data-issue-number="12"] .issue-title').click();
          await page.locator('.issue-context > summary').click();
          const historicalConversation=page.locator('.issue-context').getByRole('link',{name:'Last agent conversation'});
          assert(await historicalConversation.isVisible());
          assert.match(await historicalConversation.getAttribute('href'),/agent=codex%3Aprevious/);
          assert.equal(await page.locator('#issue-assignment').inputValue(),'unassigned');
          if(process.env.BADGE_SCREENSHOTS) await page.screenshot({path:`${process.env.BADGE_SCREENSHOTS}/${engine.name()}-${width}-history.png`,fullPage:true});
          await list();
          const alignment = await page.locator('.issue-row-end').evaluateAll(ends => ends.map(end => {
            const badge = end.querySelector('.assignment-badge')?.getBoundingClientRect();
            const comments = end.querySelector('.comment-count')?.getBoundingClientRect();
            const actions = end.querySelector('.issue-list-actions').getBoundingClientRect();
            const row = end.closest('.issue-row').getBoundingClientRect();
            return {badge:badge && badge.x-row.right, badgeRight:badge && badge.right-row.right, comments:comments && comments.x-row.right, commentsRight:comments && comments.right-row.right,
              actions:actions.x-row.right, actionsRight:actions.right-row.right, actionsCenter:actions.y+actions.height/2,
              badgeCenter:badge && badge.y+badge.height/2, commentsCenter:comments && comments.y+comments.height/2};
          }));
          const badges = alignment.filter(r => r.badge != null), comments = alignment.filter(r => r.comments != null);
          assert(Math.max(...badges.map(r=>r.badge))-Math.min(...badges.map(r=>r.badge))<1, 'Badge column aligns across empty, short and long counts');
          assert(Math.max(...comments.map(r=>r.comments))-Math.min(...comments.map(r=>r.comments))<1, 'Comments align even without an assignee');
          assert(badges.every(r=>r.commentsRight==null||r.commentsRight<=r.badge||r.badgeRight<=r.comments), 'Badge and comment controls do not overlap');
          assert(Math.max(...alignment.map(r=>r.actions))-Math.min(...alignment.map(r=>r.actions))<1, 'Actions align for assigned, unassigned and draft rows, with and without comments');
          assert(alignment.every(r=>(r.badgeRight==null||r.badgeRight<=r.actions)&&(r.commentsRight==null||r.commentsRight<=r.actions)), 'Actions consistently follow comments and assignment');
          assert(alignment.every(r=>(r.badgeCenter==null||Math.abs(r.badgeCenter-r.actionsCenter)<1)&&(r.commentsCenter==null||Math.abs(r.commentsCenter-r.actionsCenter)<1)), 'All row controls stay on one line');
          for (const scheme of ['light','dark']) {
            await page.emulateMedia({colorScheme:scheme});
            assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth), 'Mixed issue rows fit the viewport');
            if(process.env.BADGE_SCREENSHOTS) await page.screenshot({path:`${process.env.BADGE_SCREENSHOTS}/${engine.name()}-${width}-${scheme}-rows.png`,fullPage:true});
          }
          for (const number of [1,3,5,8,9,10]) {
            const trigger=page.locator(`[data-list-actions="${number}"]`);
            await trigger.scrollIntoViewIfNeeded();
            await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
            await trigger.click();
            const menu=page.getByRole('menu',{name:`Actions for issue #${number}`});
            await menu.waitFor({state:'visible'});
            const bounds=await menu.boundingBox();
            assert(bounds.x>=0&&bounds.x+bounds.width<=width, 'Every row menu fits the viewport');
            if (number===8 || number===9) await menu.getByRole('menuitem',{name:'Mark ready',exact:true}).waitFor();
            await page.keyboard.press('Escape');
            assert(await trigger.evaluate(el=>el===document.activeElement), 'Menu dismissal restores row focus');
          }
          for(const [number,owner] of [[1,'codex:astra'],[2,'human:boss'],[3,'watcher:github'],[4,'machine:box']]){
            await list();
            if(touch){
              const box=await badge(number).boundingBox();assert(box.width>=44&&box.height>=44);
              await badge(number).tap();await card(number).waitFor({state:'visible'});
              assert.equal(await page.evaluate(() => model.route.owner),'all');
              await card(number).getByRole('link',{name:/Filter by/}).tap();
            }else{
              await badge(number).hover();await card(number).waitFor({state:'visible'});
              assert.equal(await badge(number).innerText(),'');
              await badge(number).click();
            }
            await page.waitForFunction(owner=>model.route.owner===owner&&model.issues.length>0,owner);
            assert.equal(await page.evaluate(() => model.route.owner),owner);
            assert(requests.some(op=>op.action==='list'&&op.assignee===owner));
            assert(!new URL(page.url()).hash.includes('issue='));
          }
          await list();
          if(touch)await badge(1).tap();else await badge(1).hover();
          await card(1).waitFor({state:'visible'});
          assert.match(await card(1).innerText(),/Codex · gpt-6-astra/);
          assert.match(await card(1).innerText(),/Devbox/);
          assert.doesNotMatch(await card(1).innerText(),/remote attempt|Last reported/i);
          await card(1).getByText('Working',{exact:true}).waitFor();
          assert.match(await card(1).locator('.assignment-runtime').innerText(),/2m 0[5-9]s/);
          const initialTime=await card(1).locator('.assignment-runtime').innerText();
          await page.waitForFunction(text=>document.querySelector('#assignment-card-1 .assignment-runtime').textContent!==text,initialTime);
          const link=card(1).getByRole('link',{name:'Open agent conversation'});
          assert.match(await link.getAttribute('href'),/agent=codex%3Aastra/);
          if(!touch){
            await link.hover();await page.waitForTimeout(250);assert(await card(1).isVisible());
            await page.mouse.move(0,0);await card(1).waitFor({state:'hidden'});
            await badge(1).focus();await page.keyboard.press('ArrowDown');
            assert(await card(1).getByRole('link',{name:'Open agent conversation'}).evaluate(el=>el===document.activeElement));
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
          await showOwner(11);
          await card(11).getByText('Working',{exact:true}).waitFor();
          assert.match(await card(11).innerText(),/2 agents working on this task/);
          assert.doesNotMatch(await card(11).innerText(),/remote attempt|Last reported/i);
          const replicatedTime=await card(11).locator('.assignment-runtime').innerText();
          await page.waitForFunction(text=>document.querySelector('#assignment-card-11 .assignment-runtime').textContent!==text,replicatedTime);
          await list();
          await page.locator('[data-issue-number="1"] .list-label-filter').click();
          await page.waitForFunction(()=>model.route.label==='needs-boss');
          assert.equal(await page.evaluate(() => model.route.label),'needs-boss');
          await page.locator('[data-issue-number="1"] .issue-title').click();
          const detailTag=page.locator('.side-labels .list-label-filter');await detailTag.waitFor().catch(async e=>{throw Error(e.message+"\n"+await page.locator("#detail-view").innerText())});await detailTag.click();
          await page.waitForFunction(()=>!model.route.issue&&model.route.label==='needs-boss');
          assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
          await list();
          for(const scheme of ['light','dark']){
            await page.keyboard.press('Escape');
            await page.mouse.move(0,0);
            await page.emulateMedia({colorScheme:scheme});
            const colors=await page.locator('.assignment-badge').evaluateAll(badges=>badges.map(b=>getComputedStyle(b).color));
            assert.notEqual(colors[0],colors[1], 'Workers have a distinct green color');
            assert.notEqual(colors[3],colors[1], 'Machines have a distinct orange color');
            assert.notEqual(colors[0],colors[3], 'Machine and worker colors differ');
            if(process.env.BADGE_SCREENSHOTS)await page.screenshot({path:`${process.env.BADGE_SCREENSHOTS}/${engine.name()}-${width}-${scheme}-aligned.png`,fullPage:true});
            await showOwner(1);
            await card(1).getByText('Working',{exact:true}).waitFor();
            const box=await card(1).boundingBox();assert(box&&box.x>=0&&box.y>=0&&box.x+box.width<=width);
            if(process.env.BADGE_SCREENSHOTS)await page.screenshot({path:`${process.env.BADGE_SCREENSHOTS}/${engine.name()}-${width}-${scheme}.png`});
          }
          if(width===1440){
            for(const state of ['completed','offline','unknown','error']){
              await page.keyboard.press('Escape');
              await page.route('**/api/fleet/assignment?*',route=>route.fulfill({status:state==='error'?503:200,contentType:'application/json',body:JSON.stringify({ok:state!=='error',online:state!=='offline',machine:{state:state==='offline'?'disconnected':'connected',heartbeat:Date.now()/1000},run:{project_id:project.id,number:1,actor_id:'codex:astra',state:state==='completed'?'completed':'running',started_at:state==='unknown'?null:Date.now()-125000,finished_at:state==='completed'?Date.now()-5000:null}})}));
              await badge(1).focus();await badge(1).press('ArrowDown');
              await card(1).getByText(state==='error'?'Activity unavailable':state==='offline'?'Last seen':state==='completed'?'Completed':'Start time not recorded',{exact:true}).waitFor();
              const text=await card(1).locator('.assignment-runtime').innerText();
              if(state!=='error') {await page.waitForTimeout(1100);assert.equal(await card(1).locator('.assignment-runtime').innerText(),text);}
              if(process.env.BADGE_SCREENSHOTS)await page.screenshot({path:process.env.BADGE_SCREENSHOTS+'/'+engine.name()+'-'+state+'.png'});
              await page.unroute('**/api/fleet/assignment?*');
            }
          }
          assert.deepEqual(errors,[]);
          console.log(engine.name(),width,touch?'touch':'desktop','passed');
          await context.close();
        }
      }finally{await browser.close();}
    }
  }finally{server.closeAllConnections();await new Promise(r=>server.close(r));}
})().catch(e=>{console.error(e);process.exitCode=1;});
