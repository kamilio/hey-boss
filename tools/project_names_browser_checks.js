// Run with playwright-cli run-code against the isolated project-name fixture.
async page => {
 const base=await page.evaluate(()=>location.origin),paired=base.endsWith(':52092');
 const checks=[],errors=[];const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 page.on('pageerror',e=>errors.push(e.message));
 await page.route('**/api/inbox*',r=>r.fulfill({json:{ok:true,tasks:[]}}));
 const boot=await(await page.request.get(base+'/api/bootstrap')).json();
 check(boot.projects.filter(p=>p.name==='poe2').length===1,'One poe2 destination');
 check(boot.project_warnings.filter(w=>w.name==='poe2').length===2,'Both collisions surfaced');
 for(const path of [paired?'/issues':'/','/mm','/artifacts','/agents']){
  for(const [width,height] of [[320,568],[390,844],[844,390],[1440,1000]]){
   for(const colorScheme of ['light','dark']){
    await page.setViewportSize({width,height});await page.emulateMedia({colorScheme});
    await page.goto(base+path+'#project='+encodeURIComponent('github.com/poe-internal/poe2'),{waitUntil:'domcontentloaded',timeout:60000});
    await page.waitForFunction(()=>document.querySelector('#project-name')?.textContent==='poe2');
    const notice=page.locator('#project-name-notice');await notice.waitFor();
    await notice.locator('summary').click();
    check(await notice.locator('li').count()===2,`${path}/${width}/${colorScheme}: warnings readable`);
    check(await notice.evaluate(e=>e.scrollWidth<=e.clientWidth),`${path}/${width}/${colorScheme}: warning fits`);
    await notice.locator('summary').click();
    await page.locator('#project-trigger').click();await page.locator('#project-search').fill('poe2');
    check(await page.locator('.project-option').count()===1,`${path}/${width}/${colorScheme}: one picker choice`);
    check(!(await page.locator('.project-option').innerText()).includes('github.com'),`${path}/${width}/${colorScheme}: picker uses name`);
    const box=await page.locator('#project-menu').boundingBox();
    check(box.x>=0&&box.y>=0&&box.x+box.width<=width+1&&box.y+box.height<=height+1,`${path}/${width}/${colorScheme}: menu fits viewport`);
    await page.keyboard.press('ArrowDown');await page.keyboard.press('Enter');
    check(await page.locator('#project-menu').isHidden(),`${path}/${width}/${colorScheme}: keyboard selection`);
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${path}/${width}/${colorScheme}: no page overflow`);
   }
  }
 }
 const payloads=[];page.on('request',r=>{if(r.url().endsWith('/api/action')&&r.method()==='POST')payloads.push(r.postDataJSON());});
 for(const [width,height] of [[320,568],[390,844],[1440,1000]]){
  for(const colorScheme of ['light','dark']){
   await page.setViewportSize({width,height});await page.emulateMedia({colorScheme});
   await page.goto(base+(paired?'/issues':'/')+'#project='+encodeURIComponent('github.com/poe-internal/poe2'),{waitUntil:'domcontentloaded',timeout:60000});
   await page.locator('#quick-issue-open').click();
   const input=page.locator('#quick-issue-title');await input.fill('Verify destination @po');
   await page.locator('#quick-issue-project-picker').waitFor();
   check(await page.locator('.quick-issue-project').count()===1,`${width}/${colorScheme}: one Quick Issue suggestion`);
   await page.keyboard.press('Tab');
   check((await input.inputValue()).includes('@poe2'),`${width}/${colorScheme}: completion uses name`);
   check(!(await input.inputValue()).includes('github.com'),`${width}/${colorScheme}: no repository mention`);
   const dialog=page.locator('#quick-issue-dialog');
   check(await dialog.evaluate(e=>{const r=e.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&e.scrollWidth<=e.clientWidth;}),`${width}/${colorScheme}: Quick Issue fits`);
   if(width===390||width===1440)await page.screenshot({path:`output/playwright/issue92/${paired?'paired':'native'}-${width}-${colorScheme}.png`});
   await page.keyboard.press('Enter');await page.waitForFunction(()=>!document.querySelector('#quick-issue-dialog').open);
  }
 }
 const creates=payloads.filter(p=>p.operation?.action==='create');
 check(creates.length===6&&creates.every(p=>p.project==='poe2'),'Every Quick Issue submission uses the unique name');
 check(errors.length===0,'No browser errors: '+errors.join('; '));
 return {passed:checks.length,checks};
}
