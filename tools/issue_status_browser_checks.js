async page => {
 const goto=url=>page.goto(url,{waitUntil:"domcontentloaded"});
 await page.unrouteAll({behavior:"wait"});
 const checks=[], errors=[], historyReads=[];
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 const base='http://127.0.0.1:4796/#project=named%3AProgress%20Studio';
 page.on('pageerror',e=>errors.push(e.message));
 page.on('request',r=>{if(r.url().endsWith('/api/action')&&r.postDataJSON()?.operation.action==='status_history')historyReads.push(r.postDataJSON().operation);});
 const fits=()=>page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth);
 const elementFits=selector=>page.locator(selector).evaluateAll(els=>els.every(el=>{const r=el.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&el.scrollWidth<=el.clientWidth+1;}));
 for(const theme of ['light','dark']){
  await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
  for(const width of [1440,768,390,320]){
   await page.setViewportSize({width,height:1000});await goto(base);
   await page.locator('.issue-row').nth(5).waitFor();
   check(await page.locator('.issue-progress .progress-badge').allTextContents().then(v=>JSON.stringify(v)===JSON.stringify(['On track','At risk','In trouble','At risk','On track'])),`${theme}/${width}: all three statuses and unset omitted`);
   check(await fits()&&await elementFits('.issue-progress'),`${theme}/${width}: list fits`);
   check(await page.locator('[data-issue-number="5"] .issue-progress-comment').evaluate(el=>el.scrollWidth>el.clientWidth),`${theme}/${width}: long list update is compact`);
   await page.screenshot({path:`output/playwright/issue70/${theme}-${width}-list.png`});
   await page.locator('.issue-title').nth(0).click();await page.locator('.issue-progress-card').waitFor();
   check(!await page.locator('.progress-history').getAttribute('open'),`${theme}/${width}: history starts collapsed`);
   const reads=historyReads.length;
   check(await page.locator('.progress-comment').textContent()==='The fix passes tests. Checking the phone layout next.',`${theme}/${width}: full current update`);
   check(await fits()&&await elementFits('.issue-progress-card'),`${theme}/${width}: detail fits`);
   await page.screenshot({path:`output/playwright/issue70/${theme}-${width}-detail.png`});
   check(historyReads.length===reads,`${theme}/${width}: collapsed history does not fetch`);
   await page.locator('.progress-history summary').focus();await page.keyboard.press('Enter');
   await page.locator('.progress-history-list li').nth(19).waitFor();
   check(await page.locator('.progress-history-list li').count()===20,`${theme}/${width}: lazy history has twenty updates`);
   check(await page.locator('.progress-history-list li').first().locator('p').textContent()==='The fix passes tests. Checking the phone layout next.',`${theme}/${width}: newest first`);
   check(await page.locator('.progress-history-list').evaluate(el=>el.clientHeight<=320),`${theme}/${width}: expanded history is bounded`);
   await page.screenshot({path:`output/playwright/issue70/${theme}-${width}-history.png`});
   await page.getByRole('button',{name:'Show older updates',exact:true}).click();
   await page.locator('.progress-history-list li').nth(24).waitFor();
   check(await page.locator('.progress-history-list li').count()===25,`${theme}/${width}: older page appends`);
   check(!await page.getByRole('button',{name:'Show older updates',exact:true}).isVisible(),`${theme}/${width}: pagination ends`);
   await page.locator('.progress-history summary').click();
   check(await fits(),`${theme}/${width}: expanded history has no horizontal overflow`);
   await goto(base+'&issue=5');await page.locator('.progress-comment').waitFor();
   check((await page.locator('.progress-comment').textContent()).length===500,`${theme}/${width}: detail keeps entire 500-character update`);
   check(await fits()&&await elementFits('.progress-comment'),`${theme}/${width}: unbroken message wraps`);
   await page.screenshot({path:`output/playwright/issue70/${theme}-${width}-long.png`});
  }
 }
 await page.setViewportSize({width:1440,height:1000});await goto(base+'&issue=6');await page.locator('.progress-comment').waitFor();
 check(await page.locator('.progress-comment img').count()===0,'HTML-like status text is escaped');
 check((await page.locator('.progress-comment').textContent()).includes('<img src=x'),'Escaped status retains original text');
 await goto(base+'&issue=4');await page.locator('.progress-empty').waitFor();
 check(await page.locator('.progress-history').count()===0,'Unset detail is quiet and has no history control');
 const input=page.getByLabel('Your comment');await input.fill('Keep this lasting finding while progress updates.');
 await input.evaluate(el=>{el.focus();el.setSelectionRange(5,15);});
 const status={id:'live-first',author:'human:qa',level:'orange',comment:'Checking one failing test.',created_at:Date.now()};
 await page.route('**/api/action',async route=>{
  if(route.request().postDataJSON()?.operation.action!=='view')return route.continue();
  const response=await route.fetch(),body=await response.json();body.issue.status={...status};await route.fulfill({response,json:body});
 });
 await page.evaluate(async()=>{while(model.polling)await new Promise(r=>setTimeout(r,10));await refresh(false);});
 await page.locator('.progress-comment').waitFor();
 check(await page.locator('.issue-progress-card .progress-badge').first().textContent()==='At risk','First live update mounts history');
 check(await input.inputValue()==='Keep this lasting finding while progress updates.','First update keeps draft');
 check(await input.evaluate(el=>document.activeElement===el&&el.selectionStart===5&&el.selectionEnd===15),'First update keeps focus and selection');
 status.level='red';status.comment='The server is unavailable.';status.id='live-second';
 await page.evaluate(async()=>{while(model.polling)await new Promise(r=>setTimeout(r,10));await refresh(false);});
 check(await page.locator('.issue-progress-card .progress-badge').first().textContent()==='In trouble','Later update refreshes immediately');
 check(await input.evaluate(el=>document.activeElement===el&&el.selectionStart===5&&el.selectionEnd===15),'Later update keeps focus and selection');
 check(await page.locator('#update-banner').count().then(async n=>!n||!await page.locator('#update-banner').isVisible()),'Status alone does not ask to reload the issue');
 await page.unroute('**/api/action');
 await goto(base+'&issue=1');await page.locator('.progress-history summary').waitFor();
 await page.route('**/api/action',route=>route.request().postDataJSON()?.operation.action==='status_history'?route.fulfill({status:503,json:{ok:false,error:{message:'Connection interrupted. Try again.'}}}):route.continue());
 await page.locator('.progress-history summary').click();await page.locator('.progress-history-error').waitFor();
 check(await page.getByRole('button',{name:'Retry history',exact:true}).isVisible(),'History failure has retry');
 check(await page.locator('.progress-comment').textContent()==='The fix passes tests. Checking the phone layout next.','History failure preserves current update');
 await page.unroute('**/api/action');await page.getByRole('button',{name:'Retry history',exact:true}).click();await page.locator('.progress-history-list li').nth(19).waitFor();
 check(!await page.locator('.progress-history-error').isVisible(),'Retry recovers history');
 check(errors.length===0,`No browser errors: ${errors}`);
 return {checks:checks.length,historyReads:historyReads.length,errors};
}
