// Run against tools/serve_agents_fixture.mjs through playwright-cli run-code.
async page => {
  const shots='output/playwright/manage-workers/';
  const checks=[],requests=[],errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.on('pageerror',e=>errors.push(e.message));
  const run={id:'task',project_id:'named:Atlas',number:1,title:'Improve reconnect',state:'running',finished_at:null};
  const worker=(id,pid,directory,projects=['named:Atlas'])=>({id,pid,config:{name:'Worker 123',directory,projects,enabled:true,concurrency:2},runs:pid?[run,{...run,id:'finished',finished_at:1}]:[]});
  let snapshot={ok:true,machines:[
    {host:'local',hostname:'This Mac',state:'connected',workers:[worker('atlas-main',10,'/work/atlas'),worker('atlas-worktree',11,'/work/atlas-worktrees/a-very-long-checkout-directory-for-the-same-project'),worker('old-atlas',null,'/work/old-atlas'),worker('beacon',12,'/work/beacon',['named:Beacon'])]},
    {host:'remote',hostname:'Remote Mac',state:'disconnected',workers:[worker('remote-atlas',13,'/home/kamil/atlas')]},
  ],signals:[],conflicts:[{reason:'New offline projects require registration before disconnection'}]};
  const multi=snapshot.machines[0].workers[0];
  multi.config.directory='';multi.config.projects=['named:Atlas','named:Beacon'];
  multi.config.directories={'named:Atlas':'/work/atlas','named:Beacon':'/work/less-common-projects/beacon-with-a-long-checkout-path'};
  await page.route('**/api/fleet/status',r=>r.fulfill({json:{...snapshot,machines:snapshot.machines.map(m=>({...m,heartbeat:m.state==='connected'?Date.now()/1000:0}))}}));
  await page.route('**/api/fleet',r=>{requests.push(r.request().postDataJSON());return r.fulfill({json:{ok:true}});});
  await page.setViewportSize({width:1440,height:1000});
  await page.goto('http://127.0.0.1:59641/agents#project=named%3AAtlas',{waitUntil:'domcontentloaded'});
  if(!await page.locator('#device-settings').evaluate(e=>e.open))await page.locator('#device-settings>summary').click();
  await page.waitForSelector('.device-group');
  check(await page.locator('.device-group').count()===2,'Workers grouped by device');
  check(await page.locator('.device-row[data-worker="beacon"]').count()===0,'Unrelated project is excluded');
  check((await page.locator('.device-summary').first().innerText())==='2 running workers · 2 active agents / 4 slots','Live counts exclude history and stopped workers');
  check((await page.locator('.worker-directory code').allTextContents()).includes('/work/atlas-worktrees/a-very-long-checkout-directory-for-the-same-project'),'Full checkout directory is visible');
  check(await page.locator('[data-worker="atlas-main"] .worker-directory').count()===2,'Multi-project worker shows every explicit checkout');
  check((await page.locator('[data-worker="atlas-main"] .worker-directory').allTextContents()).every(text=>text.includes('Working directory ·')),'Each checkout is labelled with its project');
  check((await page.locator('[data-worker="atlas-main"] .worker-directory code').allTextContents()).includes(multi.config.directories['named:Beacon']),'Filtering a worker preserves its other project checkout');
  check((await page.locator('.device-row[data-worker="atlas-main"] .worker-name').innerText())!==(await page.locator('.device-row[data-worker="atlas-worktree"] .worker-name').innerText()),'Default names distinguish workers instead of old process numbers');
  check(!await page.locator('.device-row[data-worker="old-atlas"]').isVisible(),'Stopped workers are collapsed');
  check((await page.locator('.device-row[data-worker="remote-atlas"] .worker-state').innerText())==='Last seen running','Disconnected snapshot does not claim to be live');
  check(await page.locator('#conflicts').count()===0,'Raw sync warning is absent from Agents');
  if(!await page.locator('.saved-workers').evaluate(e=>e.open))await page.locator('.saved-workers>summary').click();
  check(await page.locator('.device-row[data-worker="old-atlas"] button').innerText()==='Start worker','Stopped worker offers only start');
  await page.locator('.device-row[data-worker="atlas-worktree"] [data-signal="pause"]').click();
  await page.waitForTimeout(100);
  check(requests[0].host==='local'&&requests[0].worker==='atlas-worktree'&&requests[0].signal==='pause','Control targets the identified worker');
  check(await page.locator('.saved-workers').evaluate(e=>e.open),'Refresh preserves stopped worker expansion');
  await page.screenshot({path:shots+'desktop.png',fullPage:true});
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:shots+'desktop-dark.png',fullPage:true});
  await page.emulateMedia({colorScheme:'light'});
  for(const width of [390,320]){
    await page.setViewportSize({width,height:844});
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),width+'px working directories and controls fit');
  }
  await page.screenshot({path:shots+'mobile-light.png',fullPage:true});
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:shots+'mobile-dark.png',fullPage:true});
  await page.locator('#show-all').click({noWaitAfter:true});
  await page.locator('.device-row[data-worker="beacon"]').waitFor({state:'attached'});
  if(!await page.locator('#device-settings').evaluate(e=>e.open))await page.locator('#device-settings>summary').click();
  check(await page.locator('.device-row[data-worker="beacon"]').isVisible(),'All projects restores other workers');
  check((await page.locator('#device-help').innerText()).startsWith('Workers across all projects.'),'Fleet-wide scope is explicit');
  check(errors.length===0,'No browser errors');
  return {passed:checks.length,checks};
}
