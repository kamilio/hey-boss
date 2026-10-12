// Run with playwright-cli run-code against serve_agents_navigation_fixture.mjs.
async page => {
  const checks=[],errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.on('pageerror',error=>errors.push(error.message));
  const visit=async path=>{await page.goto('http://127.0.0.1:59643'+path);await page.waitForFunction(()=>document.querySelector('#project-name').textContent!=='Projects');};
  await visit('/workers#project=named%3AAtlas');
  check(await page.locator('.fleet-tabs').count()===0,'No standalone conversation navigation');
  check(await page.locator('#worker-config').isVisible(),'Workers opens by default');
  await page.locator('#worker-config-view').click();
  await page.locator('#config-editor').waitFor({state:'visible'});
  await page.locator('#project-trigger').click();
  await page.locator('button[data-project="named:Beacon"]').click();
  await page.waitForFunction(()=>document.querySelector('#project-name').textContent==='Beacon');
  check(await page.locator('#config-editor').isVisible(),'Project switch preserves worker settings');
  check(await page.locator('#nav-workers').getAttribute('href')==='/workers','Worker destination is global');
  check(await page.locator('.app-navigation #nav-workers').count()===0,'Workers is outside project navigation');
  for(const path of ['/agents','/agents#view=conversations','/workers']) {
    await visit(path);
    check(await page.locator('#worker-config').isVisible(),path+' opens worker administration');
    check(await page.locator('#project-name').innerText()==='Beacon','Remembered project retained for project links');
  }
  await visit('/agents/session#project=named%3ABeacon&issue=42');
  check((await page.locator('#back').getAttribute('href')).includes('issue=42'),'Conversation returns to its task');
  check(!errors.length,'No browser errors');
  return {passed:checks.length,checks};
}
