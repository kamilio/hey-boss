// Run through playwright-cli run-code with serve_steering_fixture.mjs active.
async page => {
 const checks=[],errors=[],shots='output/playwright/issue113/';
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 await page.unroute('**/api/fleet/steer');
 page.on('pageerror',error=>errors.push(error.message));
 async function open(base,mobile){
  if(mobile){const pair=await(await page.request.get(base+'/fixture-pairing')).json();await page.request.post(base+'/api/pair',{data:{code:pair.code,deviceName:'Optimistic QA'}});}
  await page.goto(base+'/agents',{waitUntil:'domcontentloaded'});await page.locator('.agent-card').filter({hasText:'Keep conversations intact'}).click();
  await page.waitForSelector('#steer-open:not([hidden])');
 }
 for(const [base,mobile] of [['http://127.0.0.1:59682',false],['http://127.0.0.1:59782',true]]){
  await open(base,mobile);
  for(const [width,scheme] of [[1440,'light'],[390,'light'],[320,'dark'],[1440,'dark']]){
   await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme:scheme});await page.evaluate(()=>scrollTo(0,0));
   let release,payload,count=0;
   const gate=new Promise(resolve=>release=resolve);
   await page.route('**/api/fleet/steer',async route=>{count++;payload=route.request().postDataJSON();await gate;await route.fulfill({status:503,json:{ok:false,error:'Device disconnected. Retry your saved instruction.'}});});
   await page.locator('#steer-open').click();await page.locator('input[value=issue]').check();
   const text='Keep keyboard navigation fast '+width+' '+scheme;
   await page.locator('#steer-text').fill(text);await page.locator('#steer-send').click();
   try{
    await page.waitForFunction(()=>!document.querySelector('#steer-dialog').open,null,{timeout:1000});
    check(true,'Dialog dismisses before response '+width+' '+scheme);
    check((await page.locator('#steer-note').innerText()).includes('Sending'),'Pending status is honest');
    check(await page.locator('#main').evaluate(e=>document.activeElement===e),'Focus returns to conversation');
    check(await page.locator('#steer-open').isDisabled(),'Duplicate submissions prevented');
    await page.screenshot({path:shots+(mobile?'phone':'native')+'-'+width+'-'+scheme+'-pending.png'});
   }finally{release();}
   await page.waitForSelector('#steer-error:not([hidden])');
   check(await page.locator('#steer-dialog').isVisible(),'Failure restores dialog');
   check(await page.locator('#steer-text').inputValue()===text,'Failure retains exact draft');
   check(await page.locator('input[value=issue]').isChecked(),'Failure retains scope');
   check(await page.locator('#steer-text').evaluate(e=>document.activeElement===e),'Failure focuses draft');
   check(await page.locator('#steer-send').isEnabled(),'Failure enables retry');
   check(await page.locator('#steer-error').evaluate(e=>e.getBoundingClientRect().top>=0&&e.getBoundingClientRect().bottom<=innerHeight),'Recovery error is visible');
   check(await page.locator('#steer-text').evaluate(e=>e.getBoundingClientRect().top>=0&&e.getBoundingClientRect().bottom<=document.querySelector('#steer-send').getBoundingClientRect().top),'Recovered text stays above sticky actions');
   check(await page.locator('#steer-note').isHidden(),'Failure clears pending status');
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No page overflow');
   check(await page.locator('#steer-dialog').evaluate(e=>e.scrollWidth<=e.clientWidth+1),'No dialog overflow');
   check(await page.locator('#steer-send').evaluate(e=>e.getBoundingClientRect().height>=44&&e.getBoundingClientRect().bottom<=innerHeight),'Send stays visible with 44px target');
   await page.screenshot({path:shots+(mobile?'phone':'native')+'-'+width+'-'+scheme+'-recovered.png'});
   await page.unroute('**/api/fleet/steer');
   let retry;
   await page.route('**/api/fleet/steer',route=>{retry=route.request().postDataJSON();return route.fulfill({json:{ok:true,state:'queued'}});});
   await page.locator('#steer-send').click();await page.waitForFunction(()=>document.querySelector('#steer-note').textContent.includes('queued'));
   check(payload.request_id===retry.request_id,'Retry reuses idempotency key');check(count===1,'Only one pending request');
   check(!await page.locator('#steer-dialog').isVisible(),'Success keeps dialog dismissed');
   await page.unroute('**/api/fleet/steer');await page.locator('#steer-open').click();check(await page.locator('#steer-text').inputValue()==='','Success clears draft');await page.keyboard.press('Escape');
  }
  // A hung request must recover without an unbounded disabled UI.
  let release;
  const gate=new Promise(resolve=>release=resolve);
  await page.route('**/api/fleet/steer',async route=>{await gate;try{await route.fulfill({json:{ok:true,state:'queued'}});}catch{}});
  await page.locator('#steer-open').click();await page.locator('#steer-text').fill('Keep this timeout draft');await page.locator('#steer-send').click();
  try{await page.waitForSelector('#steer-error:not([hidden])',{timeout:20000});check((await page.locator('#steer-error').innerText()).includes('confirmation'),'Timeout describes uncertain delivery');check(await page.locator('#steer-text').inputValue()==='Keep this timeout draft','Timeout preserves draft');}finally{release();await page.unroute('**/api/fleet/steer');}
  await page.keyboard.press('Escape');
  await page.locator('#steer-open').click();await page.locator('#steer-text').fill('🔥'.repeat(8001));await page.locator('#steer-send').click();
  check(await page.locator('#steer-dialog').isVisible(),'Invalid instruction stays editable');
  check((await page.locator('#steer-error').innerText()).includes('32000'),'Byte limit explained before submission');
  await page.keyboard.press('Escape');
  // Late replies from another conversation must not reopen its modal here.
  const original=page.url();
  const snapshot=await(await page.request.get(base+'/api/fleet/status')).json();
  const other=snapshot.machines.flatMap(m=>m.workers.flatMap(w=>w.runs.map(run=>({host:m.host,run})))).find(e=>e.run.number===2);
  let finish,started;
  const waiting=new Promise(resolve=>finish=resolve),sent=new Promise(resolve=>started=resolve);
  await page.route('**/api/fleet/steer',async route=>{started();await waiting;await route.fulfill({status:503,json:{error:'Late failure'}});});
  await page.locator('#steer-open').click();await page.locator('#steer-text').fill('Draft for the original agent');await page.locator('#steer-send').click();await sent;
  await page.evaluate(entry=>location.hash=new URLSearchParams({project:entry.run.project_id,host:entry.host,run:entry.run.id}).toString(),other);
  await page.waitForFunction(()=>document.querySelector('#session-title').textContent.includes('Polish'));
  finish();await page.waitForFunction(()=>!document.querySelector('#steer-open').disabled);
  check(!await page.locator('#steer-dialog').isVisible(),'Late failure does not interrupt another conversation');
  check(await page.locator('#steer-note').isHidden(),'Other conversation has no stale sending status');
  await page.unroute('**/api/fleet/steer');
  await page.evaluate(hash=>location.hash=hash,original.slice(original.indexOf('#')));
  await page.waitForFunction(()=>document.querySelector('#session-title').textContent.includes('Keep conversations'));
  await page.locator('#steer-open').click();check(await page.locator('#steer-text').inputValue()==='Draft for the original agent','Navigation retains the failed draft for its agent');await page.keyboard.press('Escape');
 }
 check(errors.length===0,'No JavaScript errors');return {passed:checks.length,checks};
}
