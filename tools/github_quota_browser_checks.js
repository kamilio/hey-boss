// Run with playwright-cli run-code --filename against an isolated native web fixture.
async page => {
  const base = page.url().split('/').slice(0,3).join('/'), checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const project = {id:'named:Quota QA',name:'Quota recovery'};
  const labels = ['GitHub quota exhausted','GitHub authentication failed','Worker environment check failed','Approval service unavailable','GitHub permission denied','GitHub request failed'];
  const summaries = [
    'GitHub quota exhausted. Retry at the server reset. Saved work is retained.',
    'GitHub authentication failed (HTTP 401). Authenticate gh for github.com as the worker OS user.',
    'Worker environment check failed: Commit signing failed. Unlock the signing key.',
    'Approval service unavailable. This attempt will retry automatically.',
    'GitHub permission denied (HTTP 403). Check the current account’s GitHub permissions.',
    'GitHub request failed (HTTP 503).'
  ];
  const retry = Date.now() + 3_600_000;
  let runs = summaries.map((summary,i)=>({id:'quota-'+i,project_id:project.id,project_name:project.name,number:i+1,title:['Wait for GitHub quota to reset','Restore GitHub authentication','Verify local commit signing','Recover the approval service','Check GitHub permissions','Recover the GitHub connection'][i],state:'infrastructure_blocked',summary,started_at:1000+i,finished_at:2000+i,retry_at:retry,retry_count:1}));
  page.on('pageerror', error=>errors.push(error.message));
  await page.route('**/api/fleet/status',route=>route.fulfill({json:{ok:true,machines:[{host:'local',hostname:'This MacBook',state:'connected',heartbeat:Date.now()/1000,workers:[{id:'fixture',pid:123,config:{enabled:true,concurrency:5,projects:[project.id]},runs}]}]}}));
  await page.route('**/api/fleet/conversation?*',route=>route.fulfill({json:{ok:true,messages:[],cursor:0,has_more:false,availability:'available'}}));
  await page.route('**/api/fleet/events',route=>route.fulfill({contentType:'text/event-stream',body:'event: connected\ndata: {}\n\n'}));
  try {
    for (const theme of ['light','dark']) for (const width of [1440,390,320]) {
      await page.setViewportSize({width,height:900});
      await page.emulateMedia({colorScheme:theme});
      await page.goto(base+'/agents#view=conversations&scope=all');
      await page.waitForSelector('.is-held');
      const cards = page.locator('.is-held:visible');
      check(await cards.count()===6,`${theme} ${width}: six distinct pending failures`);
      for (let i=0;i<labels.length;i++) {
        const card = cards.nth(i), text = await card.innerText();
        check(i===3 ? text.includes('approval service') : text.includes(labels[i]),`${theme} ${width}: ${labels[i]} is preserved`);
        check(await card.locator('.agent-preview').evaluate(e=>e.scrollHeight<=e.clientHeight+1),`${theme} ${width}: guidance ${i} is not clipped`);
        check(await card.evaluate(e=>{const r=e.getBoundingClientRect();return r.left>=0&&r.right<=document.documentElement.clientWidth;}),`${theme} ${width}: card ${i} fits`);
      }
      const quota = await cards.first().innerText();
      check(quota.includes('Automatic retry at ')&&quota.includes('slot and claim are released'),`${theme} ${width}: quota explains retry deadline and release`);
      check(!/Authenticate|setup|Approval/.test(quota),`${theme} ${width}: quota has no misleading advice`);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme} ${width}: overview has no horizontal overflow`);
      await page.screenshot({path:`/tmp/hey-boss-quota-qa/overview-${theme}-${width}.png`,fullPage:true});
      await cards.first().focus();
      await page.keyboard.press('Enter');
      await page.waitForSelector('#session-page:not([hidden])');
      await page.waitForFunction(()=>document.getElementById('session-status').textContent.includes('GitHub quota exhausted'));
      const status = await page.locator('#session-status').innerText();
      check(status.includes('GitHub quota exhausted')&&status.includes('Automatic retry at '),`${theme} ${width}: keyboard opens quota detail with deadline`);
      check((await page.locator('#session-state').innerText()).startsWith('Retry in '),`${theme} ${width}: relative retry timing remains visible`);
      check(await page.locator('#steer-open').isHidden(),`${theme} ${width}: ended attempts have no live steering control`);
      await page.waitForFunction(()=>document.getElementById('conversation-empty').textContent==='No agent started during this attempt.');
      check(await page.locator('#conversation-empty').isVisible(),`${theme} ${width}: preflight explains the absent conversation`);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme} ${width}: detail has no horizontal overflow`);
      await page.screenshot({path:`/tmp/hey-boss-quota-qa/detail-${theme}-${width}.png`,fullPage:true});
    }
    runs = [...runs,{...runs[0],id:'recovered',state:'running',summary:'',started_at:3000,finished_at:null,retry_at:null}];
    await page.goto(base+'/agents#view=conversations&scope=all');
    await page.waitForSelector('.agent-card:not(.history-card)');
    check(await page.locator('.is-held:visible').count()===5,'Recovery supersedes only the matching quota hold');
    await page.locator('.project-history summary').click();
    check((await page.locator('.project-history').innerText()).includes('GitHub quota exhausted'),'Recovered quota failure remains in history');
    check(errors.length===0,`No JavaScript errors: ${errors.join(', ')}`);
    return {completed:checks.length,checks};
  } finally {
    await page.goto('about:blank');
    await page.unroute('**/api/fleet/status');
    await page.unroute('**/api/fleet/conversation?*');
    await page.unroute('**/api/fleet/events');
  }
}
