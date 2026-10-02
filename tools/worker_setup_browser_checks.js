// Run against serve_auto_workers_fixture.mjs; only synthetic configuration is edited.
async page => {
  const checks=[],errors=[],requests=[];const check=(ok,label)=>{if(!ok)throw Error(label);checks.push(label);};
  page.on('pageerror',e=>errors.push(e.message));
  const project='github.com/poe-internal/poe2';
  const worker={id:'tools',intent:'running',managed:true,pid:20,active:0,config:{name:'Tools',enabled:true,concurrency:3,projects:['github.com/acme/tools']},runs:[]};
  const machine={host:'devbox',hostname:'devbox',state:'connected',configuration_error:project+': Git clone failed. Check repository access and Git authentication on this machine.',projects:{[project]:{git:'git@github.com:poe-internal/poe2.git',path:'~/projects/poe2'}},workspace:'~/projects',workers:[worker]};
  let desired=3,revision=1,failSave=false,holdSave;
  const config=()=>({ok:true,revision:String(revision),text:'machines: {}',document:{machines:{devbox:{...machine,workers:[{...worker,config:{...worker.config,concurrency:desired}}]}}}});
  await page.route('**/api/fleet/status',r=>r.fulfill({json:{ok:true,machines:[{...machine,heartbeat:Date.now()/1000,workers:[{...worker,desired_concurrency:desired}]}],signals:[]}}));
  await page.route('**/api/fleet/configuration',async r=>{
    if(r.request().method()==='POST'){
      const input=r.request().postDataJSON();requests.push(input);
      if(input.retry_project){machine.project_retries={[project]:'queued'};return r.fulfill({json:{ok:true}});}
      if(failSave)return r.fulfill({status:409,json:{ok:false,error:'Configuration changed. Reload before saving.'}});
      if(holdSave)await holdSave;
      if(input.worker_update)desired=input.worker_update.config.concurrency;
      if(input.machine_update){const u=input.machine_update;machine.projects[u.project]={git:u.git,path:u.path};}
      revision++;
    }
    return r.fulfill({json:config()});
  });
  await page.setViewportSize({width:1440,height:950});await page.goto('http://127.0.0.1:59688/workers');
  const retry=()=>page.locator('[data-retry-project]'),minus=()=>page.locator('[data-capacity-delta="-1"]');
  await retry().waitFor();
  check((await page.locator('.setup-title').innerText()).includes('poe2'),'Failure names the project without a raw repository-ID wall');
  check(await page.getByRole('button',{name:'Edit checkout',exact:true}).isVisible(),'Checkout editing is directly available');
  check(!await page.locator('.setup-detail').evaluate(e=>e.open),'Technical details are collapsed');
  await retry().click();await page.waitForFunction(()=>document.querySelector('[data-retry-project]').disabled);
  check(requests.at(-1).retry_project.host==='devbox'&&requests.at(-1).retry_project.project===project,'Retry targets the failed checkout on its owning machine');
  await page.waitForFunction(()=>/waiting/i.test(document.querySelector('.setup-status')?.textContent||''));
  check(await retry().isDisabled(),'Retry shows a pending acknowledgement');
  await page.getByRole('button',{name:'Edit checkout',exact:true}).click();await page.locator('#machine-editor').waitFor({state:'visible'});
  check(await page.locator('#machine-editor-title').innerText()==='Edit checkout','Edit opens a checkout-specific form');
  check(await page.locator('#machine-git').inputValue()==='git@github.com:poe-internal/poe2.git','Saved Git URL is prefilled');
  check(await page.locator('#machine-path').inputValue()==='~/projects/poe2','Saved target path is prefilled');
  check(!await page.locator('#machine-worker-field').isVisible(),'Editing checkout does not silently reassign worker scope');
  await page.locator('#machine-path').fill('~/Workspace/poe2');await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  check(requests.some(r=>r.machine_update?.action==='edit-project'&&r.machine_update.project===project&&r.machine_update.path==='~/Workspace/poe2'),'Edit saves the existing project identity and updated path');
  machine.configuration_error=null;machine.project_retries={};
  let release;holdSave=new Promise(r=>release=r);await minus().click();
  await page.waitForFunction(()=>document.querySelector('.capacity-status')?.textContent==='Saving…');
  check(await page.locator('.worker-count').innerText()==='2','Requested limit is shown immediately during save');
  release();holdSave=null;
  await page.waitForFunction(()=>document.querySelector('.capacity-status')?.textContent==='Pending');
  check(worker.active===0&&worker.config.concurrency===3&&desired===2,'Idle capacity reduction remains pending until observed');
  await page.reload();await page.waitForFunction(()=>document.querySelector('.capacity-status')?.textContent==='Pending');
  check(await page.locator('.worker-count').innerText()==='2','Pending state survives a reload');
  for(const theme of ['light','dark'])for(const width of [1440,390,320]){
    await page.emulateMedia({colorScheme:theme});await page.setViewportSize({width,height:900});
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),theme+' '+width+' pending controls fit');
  }
  machine.configuration_error=project+': Git clone failed: the background service could not authenticate with SSH.';
  await page.reload();await page.waitForFunction(()=>document.querySelector('.capacity-status')?.textContent==='Blocked');
  await page.screenshot({path:'/tmp/hb-setup-mobile.png',fullPage:true});
  await page.setViewportSize({width:1440,height:950});await page.emulateMedia({colorScheme:'light'});await page.screenshot({path:'/tmp/hb-setup-desktop.png',fullPage:true});
  machine.configuration_error=null;machine.state='disconnected';await page.reload();await page.waitForFunction(()=>document.querySelector('.capacity-status')?.textContent==='Queued');
  machine.state='connected';worker.config.concurrency=2;worker.active=3;await page.reload();await page.waitForFunction(()=>document.querySelector('.capacity-status')?.textContent==='Finishing');
  worker.active=0;await page.reload();await minus().waitFor();check(await page.locator('.capacity-status').count()===0,'Applied idle limit clears pending status');
  failSave=true;await minus().click();await page.waitForFunction(()=>document.querySelector('#error').textContent.includes('Configuration changed'));
  check(await page.locator('.worker-count').innerText()==='2','Rejected save restores the confirmed limit');
  check(await minus().isEnabled(),'Rejected save leaves controls usable');
  check(errors.length===0,'No browser exceptions');return {passed:checks.length,checks};
}
