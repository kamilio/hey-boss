// Run through playwright-cli run-code with serve_live_prompt_fixture.mjs active.
async page => {
 const checks=[],errors=[];
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 page.on('pageerror',error=>errors.push(error.message));
 // Restarted local fixtures retain their origin. Routing disables Chrome's
 // HTTP cache so each review reads the actual build being served.
 await page.route('**/*',route=>route.continue());
 const shots='output/playwright/issue83/';
 const project='named:Prompt Studio';
 const runId=Date.now();
 const ready=()=>page.waitForFunction(()=>document.querySelector('#project-settings-dialog').open && !document.querySelector('#project-prompt').disabled && document.querySelector('#project-instructions-preview').getAttribute('aria-busy')==='false');
 async function review(base,mobile){
  if(mobile){
   await page.goto(base);
   const {code}=await (await page.request.get(base+'/fixture/pairing')).json();
   const paired=await page.request.post(base+'/api/pair',{data:{code,deviceName:'Prompt QA phone'}});
   check(paired.ok(),'Paired phone fixture');
  }
  for(const [width,scheme] of [[1440,'light'],[390,'light'],[320,'dark'],[1440,'dark']]){
   await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme:scheme});
   await page.goto(base+'/issues?qa='+Date.now()+'#project='+encodeURIComponent(project));
   await page.waitForFunction(()=>typeof model!=='undefined' && model.project?.id==='named:Prompt Studio' && document.querySelector('#project-settings-trigger').onclick);
   await page.locator('#project-settings-trigger').click();
   await ready();
   check(await page.locator('#project-prompt-live-help').isVisible(),`${mobile?'Phone':'Desktop'} ${width} ${scheme}: live-update explanation visible`);
   check((await page.locator('#project-prompt-live-help').innerText()).includes('keep their progress and session'),'Explains preserved progress');
   check(await page.locator('#project-prompt').getAttribute('aria-describedby')==='project-prompt-live-help','Instructions have accessible help');
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No page horizontal overflow');
   check(await page.locator('#project-settings-dialog').evaluate(e=>e.scrollWidth<=e.clientWidth+1),'Settings fit viewport');
   await page.screenshot({path:shots+`${mobile?'phone':'desktop'}-${width}-${scheme}.png`,fullPage:true});
   await page.locator('#project-prompt').fill('Claim and implement `{{issue_command}}`.\nPreserve the task and verify the new instructions.');
   await page.waitForFunction(()=>document.querySelector('#project-instructions-preview').textContent.includes('Preserve the task'));
   check((await page.locator('#project-instructions-preview').innerText()).includes('hey-boss issue view'),'Preview expands the actual issue command');
   await page.locator('#project-settings-cancel').click();
   await page.locator('#project-settings-trigger').click();
   await ready();
   check(!(await page.locator('#project-prompt').inputValue()).includes('verify the new instructions'),'Cancel leaves saved instructions unchanged');
   await page.keyboard.press('Escape');
   check(!await page.locator('#project-settings-dialog').isVisible(),'Escape closes settings');
   check(await page.locator('#project-settings-trigger').evaluate(e=>e===document.activeElement),'Focus returns to settings trigger');
  }
  await page.locator('#project-settings-trigger').click();
  await ready();
  const text=`Claim and implement \`{{issue_command}}\`.\n${mobile?'Phone':'Desktop'} saved instruction update ${runId}.`;
  await page.locator('#project-prompt').fill(text);
  await page.locator('#project-settings-form button[type=submit]').click();
  await page.waitForFunction(()=>!document.querySelector('#project-settings-dialog').open);
  await page.waitForFunction(()=>!document.querySelector('#toast').hidden && document.querySelector('#toast').textContent.includes('Instruction updates queued'));
  check((await page.locator('#toast').innerText()).includes('Instruction updates queued'),'Save confirms queued updates without claiming delivery');
  await page.screenshot({path:shots+`${mobile?'phone':'desktop'}-saved.png`,fullPage:true});
  await page.locator('#project-settings-trigger').click();
  await ready();
  check(await page.locator('#project-prompt').inputValue()===text,'Saved instructions persist on reopening');
  await page.locator('#project-prompt-main').fill('Review the result and commit your changes.');
  await page.route('**/api/action',async route=>{
   if(route.request().postDataJSON()?.operation?.action==='configure_project')return route.fulfill({status:409,json:{ok:false,error:{message:'Settings changed elsewhere. Reload before saving.'}}});
   return route.continue();
  });
  await page.locator('#project-settings-form button[type=submit]').click();
  await page.waitForSelector('#project-settings-error:not([hidden])');
  check(await page.locator('#project-settings-dialog').isVisible(),'Failed save retains dialog');
  check(await page.locator('#project-prompt-main').inputValue()==='Review the result and commit your changes.','Failed save retains draft');
  await page.screenshot({path:shots+`${mobile?'phone':'desktop'}-conflict.png`,fullPage:true});
  await page.unroute('**/api/action');await page.locator('#project-settings-cancel').click();
 }
 await review('http://127.0.0.1:4883',false);
 await review('http://127.0.0.1:5283',true);
 check(errors.length===0,'No JavaScript errors');
 await page.unroute('**/*');
 return {passed:checks.length,checks};
}
