async page => {
 const goto=url=>page.goto(url,{waitUntil:"domcontentloaded"});
 const checks=[],errors=[];const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 await page.unrouteAll({behavior:'wait'});page.on('pageerror',e=>errors.push(e.message));
 const base='http://127.0.0.1:52070';
 await goto(base+'/');
 const {code}=await (await page.request.get(base+'/fixture/pairing')).json();
 const paired=await page.request.post(base+'/api/pair',{data:{code}});check(paired.ok(),'Fixture device paired');
 for(const theme of ['light','dark'])for(const width of [390,320,768]){
  await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
  await goto(base+`/project-resource?qa=${theme}-${width}#project=named%3AProgress%20Studio&issue=1`);
  await page.locator('.issue-progress-card').waitFor();
  check(await page.locator('.progress-comment').textContent()==='The fix passes tests. Checking the phone layout next.',`${theme}/${width}: paired viewer shows current update`);
  check(await page.locator('.progress-history').getAttribute('open')===null,`${theme}/${width}: paired history collapsed`);
  check(await page.locator('.issue-progress-card textarea,.issue-progress-card select').count()===0,`${theme}/${width}: paired status read-only`);
  check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme}/${width}: paired detail fits`);
  await page.screenshot({path:`output/playwright/issue70/mobile-${theme}-${width}-detail.png`});
  await page.locator('.progress-history summary').focus();await page.keyboard.press('Enter');await page.locator('.progress-history-list li').nth(19).waitFor();
  check(await page.locator('.progress-history-list li').count()===20,`${theme}/${width}: history through native bridge`);
  await page.getByRole('button',{name:'Show older updates',exact:true}).click();await page.locator('.progress-history-list li').nth(24).waitFor();
  check(await page.locator('.progress-history-list li').count()===25,`${theme}/${width}: bridge pagination`);
  await page.screenshot({path:`output/playwright/issue70/mobile-${theme}-${width}-history.png`});
 }
 check(errors.length===0,`No paired-device script errors: ${errors}`);return {checks:checks.length,errors};
}
