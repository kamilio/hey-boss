// Run with playwright-cli run-code against serve_agents_navigation_fixture.mjs.
async page => {
  const base='http://127.0.0.1:59643', checks=[], errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.on('pageerror',error=>errors.push(error.message));
  const visit=async path=>{await page.goto(base+path);await page.waitForFunction(()=>document.querySelector('#project-name').textContent!=='Projects');};
  const saved=()=>page.evaluate(()=>JSON.parse(localStorage.getItem('hey-boss-issues-project')));
  const project=()=>page.locator('#project-name').innerText();
  await page.goto('about:blank');
  await visit('/agents#project=named%3AAtlas');
  check(await page.locator('.fleet-tabs a').allTextContents().then(v=>v.join(',')==='Workers,Conversations'),'Workers first, Conversations second');
  check(await page.locator('#worker-config').isVisible(),'Workers opens by default');
  check(await project()==='Atlas','Explicit project selected');
  await page.locator('#conversations-tab').click();await page.locator('#projects').waitFor({state:'visible'});
  check(await page.locator('#projects').isVisible(),'Second tab opens conversations');
  check(await project()==='Atlas' && await saved()==='named:Atlas','Tab change preserves selected project');
  await page.locator('#project-trigger').click();
  await page.locator('button[data-project="named:Beacon"]').click();
  await page.waitForFunction(()=>document.querySelector('#project-name').textContent==='Beacon');
  check(await page.locator('#projects').isVisible(),'Project switch preserves Conversations tab');
  await page.locator('#workers-tab').click();await page.locator('#worker-config').waitFor({state:'visible'});
  check(await page.locator('#worker-config').isVisible(),'First tab returns to Workers');
  check(await project()==='Beacon','Workers retains project');
  await visit('/agents');
  check(await project()==='Beacon' && await saved()==='named:Beacon','Bare Agents URL restores saved project instead of home');
  await page.locator('#worker-config-view').click();await page.locator('#config-editor').waitFor({state:'visible'});
  check(await page.locator('#config-editor').isVisible(),'Worker settings remain accessible');
  check(await project()==='Beacon','Worker settings retain project');
  await page.locator('#project-trigger').click();
  await page.locator('button[data-project="named:Atlas"]').click();
  await page.waitForFunction(()=>document.querySelector('#project-name').textContent==='Atlas');
  check(await page.locator('#config-editor').isVisible(),'Project switch preserves worker settings');
  await visit('/agents/session#project=named%3ABeacon');
  await page.locator('#back').click();await page.locator('#projects').waitFor({state:'visible'});
  check(await page.locator('#projects').isVisible() && await project()==='Beacon','Session back returns to conversations in the same project');
  await visit('/workers');
  check(await project()==='Beacon','Legacy Workers URL restores saved project');
  await visit('/agents#project=named%3AAtlas&view=conversations');
  check(await project()==='Atlas' && await saved()==='named:Atlas','Explicit deep link overrides remembered project');
  await page.locator('#show-all').click();
  check(await page.locator('#projects').isVisible() && new URL(page.url()).hash.includes('scope=all'),'All projects remains a conversations view');
  check(await saved()==='named:Atlas','All projects does not overwrite selected project');
  await page.reload();
  check(await page.locator('#show-all').isHidden(),'All-projects scope survives reload');
  await visit('/agents#project=named%3AUnavailable');
  check(await saved()==='named:Unavailable','Unavailable project never silently switches to home');
  await visit('/agents#project=named%3ABeacon');
  for(const id of ['nav-issues','nav-artifacts','nav-mindmaps','nav-workers']) check((await page.locator('#'+id).getAttribute('href')).includes('project=named%3ABeacon'),id+' retains project');
  for(const width of [1280,390,320]) {
    await page.setViewportSize({width,height:844});
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),width+'px Workers fits');
    await page.locator('#conversations-tab').click();await page.locator('#projects').waitFor({state:'visible'});
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),width+'px Conversations fits');
    await page.locator('#workers-tab').click();await page.locator('#worker-config').waitFor({state:'visible'});
  }
  check(!errors.length,'No browser errors');
  return {passed:checks.length,checks};
}
