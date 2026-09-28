// Run with playwright-cli run-code against a native issue UI; API data is synthetic.
async page => {
  const base = page.url().split('/').slice(0,3).join('/');
  const checks = [], errors = [], mutations = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const project = {id:'named:Transport QA',name:'Transport QA'};
  const jobs = Array.from({length:7}, (_,i) => ({id:'job-'+i,project_id:project.id,project_name:project.name,number:i+1,title:['Recover a delayed handshake','Preserve the current agent session','Apply queued changes after reconnect','Keep worker assignments intact','Report database contention clearly','Check connected device status','Verify progress without restarting'][i],state:'running',started_at:Date.now()-120000,finished_at:null,last_event:'Working through the existing task.'}));
  const worker = {id:'preserved-worker',pid:74974,config:{name:'Transport worker',enabled:true,concurrency:7,projects:[project.id],directory:'/work/transport'},runs:jobs};
  let state = 'connected';
  page.on('pageerror', e => errors.push(e.message));
  await page.route('**/api/bootstrap', r => r.fulfill({json:{ok:true,csrf:'synthetic',projects:[project],project}}));
  await page.route('**/api/fleet/status', r => r.fulfill({json:{ok:true,machines:[{host:'remote',hostname:'Connected MacBook',state,heartbeat:state==='connected'?Date.now()/1000:Date.now()/1000-60,error:state==='disconnected'?'Companion hello timed out after 15 seconds without protocol progress (last startup phase: database)':null,workers:[worker]}],signals:[],conflicts:[]}}));
  await page.route('**/api/fleet', r => {mutations.push(r.request().postData());return r.fulfill({status:405,json:{ok:false,error:'Read-only test'}});});
  await page.goto(base+'/agents#project='+encodeURIComponent(project.id));
  await page.reload();
  if(!await page.locator('#device-settings').evaluate(el=>el.open))await page.locator('#device-settings>summary').click();
  await page.locator('.device-row').waitFor();
  for(const colorScheme of ['light','dark']) {
    await page.emulateMedia({colorScheme});
    for(const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      for(const next of ['connected','disconnected','connected']) {
        state=next;
        await page.locator('#refresh').click();
        await page.waitForFunction(expected => document.querySelector('.device-connection')?.textContent === expected, state==='connected'?'Connected':'Disconnected');
        const label=colorScheme+' '+width+' '+state;
        check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),label+' fits viewport');
        check(await page.locator('.device-row').count()===1,label+' retains the same worker');
        check((await page.locator('.worker-capacity').innerText()).includes('7 active agents'),label+' retains seven existing jobs');
        check((await page.locator('.worker-state').innerText())===(state==='connected'?'Working':'Last seen running'),label+' distinguishes live and remembered state');
        check((await page.locator('.device-summary').innerText())===(state==='connected'?'1 running worker · 7 active agents / 7 slots':'Showing last known worker state'),label+' has honest capacity counts');
        if(state==='disconnected'&&[1440,390,320].includes(width))await page.locator('.device-group').screenshot({path:`output/playwright/issue682/${colorScheme}-${width}-disconnected.png`});
      }
      if([1440,390,320].includes(width))await page.screenshot({path:`output/playwright/issue682/${colorScheme}-${width}-recovered.png`,fullPage:true});
    }
  }
  const refresh=page.locator('#refresh');
  await refresh.focus();await page.keyboard.press('Enter');
  check(await refresh.evaluate(el=>el===document.activeElement),'Refresh preserves keyboard focus');
  check(mutations.length===0,'Visual review sends no worker controls');
  check(errors.length===0,'No browser runtime errors');
  return {passed:checks.length,checks};
}
