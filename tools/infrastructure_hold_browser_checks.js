// Run on the Agents page of an isolated fixture through playwright-cli run-code.
async page => {
  const checks=[],errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const fits=()=>page.locator('.project-heading,.project-issues,.agent-card,.page-footer').evaluateAll(elements=>elements.filter(e=>e.getClientRects().length).every(e=>{const r=e.getBoundingClientRect();return r.left>=0&&r.right<=document.documentElement.clientWidth;}));
  page.on('pageerror',e=>errors.push(e.message));
  const base=page.url().split('/').slice(0,3).join('/');
  const labels=['Database service unavailable','Model proxy unavailable','Approval service unavailable'];
  const services=['database service','model proxy','approval service'];
  const project={id:'named:Recovery',name:'Recovery'};
  let runs=labels.map((label,i)=>({id:'held-'+i,project_id:project.id,project_name:project.name,number:138+i,title:['Preserve delivery after a database disconnect','Resume work after a proxy recovery timeout','Continue after approval service recovery'][i],state:'infrastructure_blocked',summary:label+'. The saved session and checkout are retained.',started_at:1000+i,finished_at:2000+i,retry_at:Date.now()+300000,retry_count:5}));
  await page.route('**/api/fleet/status',route=>route.fulfill({json:{ok:true,machines:[{host:'local',hostname:'This MacBook',state:'connected',heartbeat:Date.now()/1000,workers:[{id:'fixture',pid:123,config:{enabled:true,concurrency:2,projects:[project.id]},runs}]}]}}));
  await page.route('**/api/fleet/conversation?*',route=>route.fulfill({json:{ok:true,messages:[{id:'0',role:'user',text:'Preserve the saved session and verify every requirement before closing.'},{id:'1',role:'assistant',text:'The service is unavailable. Your saved session and checkout are retained.'}],cursor:2,has_more:false,availability:'available'}}));
  await page.route('**/api/fleet/events',route=>route.fulfill({contentType:'text/event-stream',body:'event: connected\ndata: {}\n\n'}));
  for(const theme of ['light','dark'])for(const width of [1440,390,320]) {
    await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme:theme});
    await page.goto(base+'/agents');await page.waitForSelector('.is-held');
    check(await page.locator('.is-held:visible').count()===3,`${theme} ${width}: all outstanding holds are visible outside history`);
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=document.documentElement.clientWidth),`${theme} ${width}: overview fits`);
    check(await fits(),`${theme} ${width}: cards and actions stay inside visible bounds`);
    for(let i=0;i<3;i++) {
      const card=page.locator('.is-held:visible').nth(i);
      check((await card.innerText()).includes(services[i]),`${theme} ${width}: ${labels[i]}`);
      check((await card.innerText()).includes('retry automatically'),`${theme} ${width}: correct recovery action ${i}`);
      check(await card.locator('.agent-preview').evaluate(e=>e.scrollHeight<=e.clientHeight+1),`${theme} ${width}: recovery instructions are not clipped ${i}`);
    }
    await page.screenshot({path:`output/playwright/issue138-delivery/overview-${theme}-${width}.png`,fullPage:false});
    await page.evaluate(()=>scrollTo(0,document.documentElement.scrollHeight));
    await page.screenshot({path:`output/playwright/issue138-delivery/overview-bottom-${theme}-${width}.png`});
    await page.locator('.is-held:visible').first().focus();await page.keyboard.press('Enter');
    await page.waitForSelector('.chat-message.assistant');
    check((await page.locator('#session-state').innerText()).startsWith('Retry in '),`${theme} ${width}: keyboard opens database session with retry timing`);
    check((await page.locator('#session-status').innerText()).includes('uncertain write'),`${theme} ${width}: conversation explains reconciliation`);
    check(await page.locator('#steer-open').isHidden(),`${theme} ${width}: ended attempts cannot be steered`);
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=document.documentElement.clientWidth),`${theme} ${width}: conversation fits`);
    await page.screenshot({path:`output/playwright/issue138-delivery/session-${theme}-${width}.png`,fullPage:false});
  }
  // A resumed attempt supersedes the hold in the overview, retaining history.
  runs=[...runs,{...runs[0],id:'resumed',state:'running',started_at:3000,finished_at:null,retry_at:null}];
  await page.goto(base+'/agents');await page.waitForSelector('.agent-card:not(.history-card)');
  check(await page.locator('.is-held:visible').count()===2,'Resuming one issue clears only its outstanding hold');
  await page.locator('.project-history summary').click();
  check(await page.locator('.is-held:visible').count()===5,'Saved holds remain accessible in expanded history');
  await page.setViewportSize({width:360,height:900});
  runs=runs.map(run=>({...run,project_name:'DatabaseAndProxyRecovery'.repeat(4)}));
  await page.goto(base+'/agents');await page.waitForSelector('.is-held');
  check(await fits(),'Long project names keep retry cards and issue navigation visible');
  check(await page.locator('.project-heading').first().evaluate(e=>e.querySelector('.project-issues').getBoundingClientRect().top>=e.querySelector('h2').getBoundingClientRect().bottom),'Narrow headings give long project names the full row before issue navigation');
  await page.screenshot({path:'output/playwright/issue138-delivery/long-project.png',fullPage:false});
  check(errors.length===0,'No JavaScript errors');
  await page.goto('about:blank');
  await page.unroute('**/api/fleet/status');await page.unroute('**/api/fleet/conversation?*');await page.unroute('**/api/fleet/events');
  if(checks.length!==101)throw Error("Incomplete visual task graph");
  return {completed:checks.length,expected:101,checks};
}
