// Run through playwright-cli run-code with serve_steering_fixture.mjs active.
async page => {
 const checks=[],errors=[],shots='output/playwright/issue82/';
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 page.on('pageerror',e=>errors.push(e.message));
 await page.unroute('**/api/fleet/steer');await page.unroute('**/api/fleet/status');await page.unroute('**/*');
 await page.route('**/*',route=>route.continue());
 const native='http://127.0.0.1:59682',phone='http://127.0.0.1:59782';
 const action=async operation=>{const b=await(await page.request.get(native+'/api/bootstrap')).json();const r=await page.request.post(native+'/api/action',{headers:{'X-Hey-Boss-CSRF':b.csrf},data:{project:'named:Steering Studio',operation,request_id:null}});const value=await r.json();if(!r.ok()||value.ok===false)throw Error(JSON.stringify(value));return value;};
 const stamp=Date.now();
 async function review(base,mobile){
  if(mobile){const pair=await(await page.request.get(base+'/fixture-pairing')).json();check((await page.request.post(base+'/api/pair',{data:{code:pair.code,deviceName:'Steering QA'}})).ok(),'Phone paired');}
  await page.goto(base+'/agents',{waitUntil:'domcontentloaded'});await page.waitForSelector('.agent-card');
  await page.locator('.agent-card').filter({hasText:'Keep conversations intact'}).click();await page.waitForSelector('#steer-open:not([hidden])');
  check(await page.locator('#takeover-open').isVisible(),'Steer alongside Take over');
  for(const [width,scheme] of [[1440,'light'],[390,'light'],[320,'dark'],[1440,'dark']]){
   await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme:scheme});await page.evaluate(()=>scrollTo(0,0));
   await page.locator('#steer-open').click();
   check(await page.locator('.steer-scopes input').count()===3,'All three scopes available');
   check(await page.locator('#steer-text').evaluate(e=>document.activeElement===e),'Keyboard focus starts in message');
   await page.locator('input[value=session]').check();await page.locator('input[value=session]').focus();await page.keyboard.press('ArrowDown');check(await page.locator('input[value=issue]').isChecked(),'Scope switches with keyboard');await page.locator('#steer-text').focus();
   await page.locator('#steer-text').fill('Also verify long task titles, keyboard navigation, and device reconnects.');
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Page fits '+width+'px '+scheme);
   check(await page.locator('#steer-dialog').evaluate(e=>e.scrollWidth<=e.clientWidth+1),'Dialog fits '+width+'px '+scheme);
   check(await page.locator('#steer-send').evaluate(e=>e.getBoundingClientRect().height>=44),'Send has a 44px target');
   check(await page.locator('#steer-send').evaluate(e=>e.getBoundingClientRect().top>=0&&e.getBoundingClientRect().bottom<=innerHeight),'Send remains visible '+width+'px');
   await page.screenshot({path:shots+(mobile?'phone':'native')+'-'+width+'-'+scheme+'.png',fullPage:false});
   await page.keyboard.press('Escape');check(!await page.locator('#steer-dialog').isVisible(),'Escape dismisses');
   await page.waitForFunction(()=>document.activeElement===document.querySelector('#steer-open'));check(await page.locator('#steer-open').evaluate(e=>document.activeElement===e),'Focus returns to Steer');
   await page.locator('#steer-open').click();check((await page.locator('#steer-text').inputValue()).includes('keyboard navigation'),'Cancelled draft retained');
   await page.locator('#steer-cancel').click();
  }
  await page.setViewportSize({width:1440,height:900});
  for(const scope of ['session','issue','project']){
   await page.locator('#steer-open').click();await page.locator('input[name=scope][value='+scope+']').check();
   const text=(mobile?'Phone':'Native')+' '+scope+' instruction '+stamp;
   await page.locator('#steer-text').fill(text);await page.locator('#steer-send').click();
   await page.waitForFunction(()=>!document.querySelector('#steer-dialog').open);
   check(!(await page.locator('#steer-note').innerText()).includes('delivered'),'Acceptance accurately says queued');
   await page.waitForFunction(text=>[...document.querySelectorAll('#steering-list li')].some(e=>e.textContent.includes(text)&&e.textContent.includes('Delivered')),text,{timeout:15000});
   check(true,scope+' instruction delivered through '+(mobile?'paired bridge':'native fleet'));
   if(scope==='issue'){const issue=(await action({action:'view',number:1})).issue;check(issue.body.includes(text),'Issue scope saved in requirements');}
   if(scope==='project'){const settings=await action({action:'project_settings'});check(settings.prompt.includes(text),'Project scope saved in base instructions');const snapshot=await(await page.request.get(native+'/api/fleet/status')).json();const other=snapshot.machines.flatMap(m=>m.workers.flatMap(w=>w.runs)).find(r=>r.number===2);let applied=false;for(let i=0;i<30&&!applied;i++){const conversation=await(await page.request.get(native+'/api/fleet/conversation?host=local&run='+encodeURIComponent(other.id)+'&latest=1')).json();applied=conversation.messages.some(m=>m.text.includes('Project instructions have changed')&&m.text.includes(text));if(!applied)await page.waitForTimeout(200);}check(applied,'Project update reaches the other running agent');}
  }
  await page.locator('#steering-updates summary').click();await page.screenshot({path:shots+(mobile?'phone':'native')+'-delivered.png',fullPage:false});
 }
 await review(native,false);await review(phone,true);
 await page.goto(native+'/agents',{waitUntil:'domcontentloaded'});await page.waitForSelector('.agent-card');await page.locator('.agent-card').filter({hasText:'Keep conversations intact'}).click();await page.waitForSelector('#steer-open:not([hidden])');
 // Lose the response after the server saves the requirement. Retrying must
 // preserve the request ID and append the saved requirement exactly once.
 let lost;await page.route('**/api/fleet/steer',async route=>{lost=route.request().postDataJSON();await route.fetch();await route.abort('failed');});
 await page.locator('#steer-open').click();await page.locator('input[value=issue]').check();const retry='Retry exactly once '+stamp;await page.locator('#steer-text').fill(retry);await page.locator('#steer-send').click();await page.waitForSelector('#steer-error:not([hidden])');
 check(await page.locator('#steer-text').inputValue()===retry,'Lost response retains instruction');
 await page.screenshot({path:shots+'lost-response.png',fullPage:false});
 await page.unroute('**/api/fleet/steer');let retried;await page.route('**/api/fleet/steer',route=>{retried=route.request().postDataJSON();return route.continue();});
 await page.locator('#steer-send').click();await page.waitForFunction(()=>!document.querySelector('#steer-dialog').open);check(lost.request_id===retried.request_id,'Retry reuses request ID');await page.unroute('**/api/fleet/steer');
 const body=(await action({action:'view',number:1})).issue.body;check(body.split(retry).length===2,'Lost response retry saved requirement once');
 await page.locator('#steer-open').click();await page.locator('input[value=session]').check();await page.locator('#steer-text').fill('REJECT synthetic instruction '+stamp);await page.locator('#steer-send').click();await page.waitForFunction(()=>!document.querySelector('#steer-dialog').open);
 await page.waitForFunction(()=>[...document.querySelectorAll('#steering-list li')].some(e=>e.textContent.includes('Not delivered')&&e.textContent.includes('REJECT')),null,{timeout:15000});
 check(true,'Rejected delivery is visible');await page.locator('#steering-updates').evaluate(e=>e.open=true);await page.screenshot({path:shots+'rejected-delivery.png',fullPage:false});
 const snapshot=await(await page.request.get(native+'/api/fleet/status')).json();
 await page.route('**/api/fleet/status',route=>route.fulfill({json:{...snapshot,machines:snapshot.machines.map(m=>({...m,state:'disconnected'}))}}));
 await page.locator('#refresh').click();await page.waitForFunction(()=>document.querySelector('#steer-open').disabled);check(await page.locator('#steer-open').isDisabled(),'Offline agent cannot be steered');
 await page.unroute('**/api/fleet/status');
 const ended=JSON.parse(JSON.stringify(snapshot));for(const machine of ended.machines)for(const worker of machine.workers)for(const run of worker.runs)run.finished_at=Date.now();
 await page.route('**/api/fleet/status',route=>route.fulfill({json:ended}));await page.locator('#refresh').click();await page.waitForFunction(()=>document.querySelector('#steer-open').hidden);check(!await page.locator('#steer-open').isVisible(),'Completed agent has no steer action');
 await page.unroute('**/api/fleet/status');check(errors.length===0,'No JavaScript errors');await page.unroute('**/*');
 return {passed:checks.length,checks};
}
