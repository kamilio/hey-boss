// Run with playwright-cli run-code and serve_status_dot_fixture.mjs.
async page => {
 const checks=[],errors=[];
 page.setDefaultNavigationTimeout(90000);
 page.setDefaultTimeout(60000);
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 await page.unrouteAll({behavior:'wait'});
 page.on('pageerror',error=>errors.push(error.message));
 await page.route('**/*',route=>route.continue());
 const expected=['On track','At risk','In trouble','At risk','On track'];
 const fits=locator=>locator.evaluateAll(elements=>elements.every(el=>{const r=el.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&el.scrollWidth<=el.clientWidth+1;}));
 const fixture='http://127.0.0.1:52094';
 await page.goto(fixture);
 const {code}=await (await page.request.get(fixture+'/fixture/pairing')).json();
 check((await page.request.post(fixture+'/api/pair',{data:{code}})).ok(),'Paired device connected');
 for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
  await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
  const mode='desktop',base=`http://127.0.0.1:4798/?qa=${theme}-${width}-${Date.now()}#project=named%3AStatus%20Dot%20QA`;
  await page.goto(base,{waitUntil:'commit'});await page.locator('.issue-row').nth(5).waitFor();
  const dots=page.locator('.issue-meta .issue-progress');
  check(await dots.count()===5,`${mode}/${theme}/${width}: five dots beside numbers, unset omitted`);
  check(await dots.evaluateAll(els=>els.every(el=>el.getAttribute('role')==='img')),'Status icons have accessible semantics');
  check(await dots.evaluateAll(els=>els.every(el=>el.previousElementSibling?.classList.contains('issue-number'))),'Dots immediately follow ticket numbers');
  check(await page.locator('.issue-progress-comment,.issue-progress .progress-badge').count()===0,'No status line or text badge in list');
  await page.mouse.move(0,0);
  check(await dots.evaluateAll(els=>els.every(el=>el.getBoundingClientRect().width<=24&&el.innerText==='')),'Only a compact dot is visible');
  check(await dots.evaluateAll((els,expected)=>els.every((el,i)=>el.getAttribute('aria-label').startsWith(expected[i])),expected),'Status levels have accessible names');
  check(await fits(page.locator('.issue-row'))&&await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'List fits viewport');
  const first=dots.first(),help=first.locator('.issue-progress-help');
  await first.hover();check(await help.isVisible(),'Hover reveals full status');
  check((await help.textContent()).includes('The fix passes tests. Checking the phone layout next.'),'Tooltip keeps whole message');
  check(await fits(help),'First tooltip fits viewport');
  check(await help.evaluate(el=>{const r=el.getBoundingClientRect();return el.contains(document.elementFromPoint(r.left+r.width/2,r.top+r.height/2));}),'Tooltip is not clipped or covered');
  await help.hover();check(await help.isVisible(),'Tooltip remains open under pointer');
  await first.focus();await page.mouse.move(0,0);check(await help.isVisible(),'Keyboard focus reveals status');
  await page.screenshot({path:`output/playwright/issue94/${mode}-${theme}-${width}-tooltip.png`});
  await page.keyboard.press('Escape');check(!await help.isVisible(),'Escape dismisses tooltip');
  await page.locator('.issue-title').first().focus();check(!await help.isVisible(),'Moving focus closes tooltip');
  const long=page.locator('[data-issue-number="5"] .issue-progress');await long.hover();
  check((await long.locator('.issue-progress-help').textContent()).includes('A'.repeat(500)),'All 500 characters retained');
  check(await fits(long.locator('.issue-progress-help')),'Unbroken message wraps');
  await page.screenshot({path:`output/playwright/issue94/${mode}-${theme}-${width}-long.png`});
  await page.mouse.move(0,0);await page.locator('[data-issue-number="6"] .issue-progress').hover();
  check(await page.locator('.issue-progress img').count()===0,'HTML-like message is escaped');
  await page.mouse.move(0,0);
  await page.locator('.issue-title').first().click();await page.locator('.progress-comment').waitFor();
  check(await page.locator('.progress-comment').textContent()==='The fix passes tests. Checking the phone layout next.','Detail preserves full status');
  await page.locator('.progress-history summary').click();await page.locator('.progress-history-list li').waitFor();
  check(await page.locator('.progress-history-list li p').textContent()==='The fix passes tests. Checking the phone layout next.','History stays intact');
 }
 // Paired devices use the shared detail card rather than the desktop list.
 for(const theme of ['light','dark'])for(const width of [390,320,768]){
  await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:theme});
  await page.goto(fixture+`/project-resource?qa=${theme}-${width}-${Date.now()}#project=named%3AStatus%20Dot%20QA&issue=1`,{waitUntil:'commit'});
  await page.locator('.progress-comment').waitFor();
  check(await page.locator('.progress-comment').textContent()==='The fix passes tests. Checking the phone layout next.',`paired/${theme}/${width}: full status preserved`);
  await page.locator('.progress-history summary').click();await page.locator('.progress-history-list li').waitFor();
  check(await page.locator('.progress-history-list li p').textContent()==='The fix passes tests. Checking the phone layout next.','Paired history preserved');
  check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Paired detail fits viewport');
  await page.screenshot({path:`output/playwright/issue94/paired-${theme}-${width}-detail.png`});
 }
 await page.goto('http://127.0.0.1:4798/#project=named%3AStatus%20Dot%20QA');
 await page.locator('.issue-progress').first().focus();await page.mouse.move(0,0);
 await page.route('**/api/action',async route=>{
  if(route.request().postDataJSON()?.operation.action!=='list')return route.continue();
  const response=await route.fetch(),body=await response.json();
  body.issues[0].status={...body.issues[0].status,level:'red',comment:'A fresh update arrived.',author:'human:previous'};
  await route.fulfill({response,json:body});
 });
 await page.evaluate(async()=>{while(model.polling)await new Promise(r=>setTimeout(r,10));await refresh(false);});
 const refreshed=page.locator('.issue-progress').first();
 check(await refreshed.evaluate(el=>el===document.activeElement),'Live list refresh keeps dot focus');
 check((await refreshed.getAttribute('aria-label')).startsWith('In trouble · A fresh update arrived.'),'Live list refresh updates dot and message');
 check((await refreshed.locator('.issue-progress-help').textContent()).includes('Previous owner'),'Tooltip preserves previous-owner context');
 check(await refreshed.locator('.issue-progress-help').isVisible(),'Tooltip stays available after refresh');
 await page.unroute('**/api/action');
 const touch=await page.context().browser().newContext({viewport:{width:390,height:844},isMobile:true,hasTouch:true});
 try {
  const phone=await touch.newPage();await phone.goto('http://127.0.0.1:4798/#project=named%3AStatus%20Dot%20QA');
  const dot=phone.locator('.issue-progress').first();await dot.tap();
  check(await dot.locator('.issue-progress-help').isVisible(),'Touch reveals full status without opening the issue');
 } finally {await touch.close();}
 check(errors.length===0,`No browser errors: ${errors}`);
 return {checks:checks.length,errors};
}
