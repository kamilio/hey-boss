// Run with playwright-cli run-code --filename against serve_auto_workers_fixture.mjs.
async page => {
  const checks=[],errors=[],requests=[];const check=(ok,label)=>{if(!ok)throw Error(label);checks.push(label);};
  page.on('pageerror',e=>errors.push(e.message));
  const project='github.com/acme/atlas',other='github.com/acme/tools';
  const worker=(id,name,projects=[project],concurrency=5)=>({id,intent:'running',managed:true,pid:20,active:id==='atlas'?5:1,config:{name,enabled:true,concurrency,projects,provider:'claude',directory:'/work/'+id,tags:['ready']},runs:[],chiefs:[]});
  let revision=1,conflict=false;
  const local={host:'local',hostname:'MacBook Pro',workers:[worker('atlas','Atlas'),worker('tools','Tools',[other],1)],projects:{[project]:{git:'git@github.com:acme/atlas.git',path:'~/Workspace/atlas'}}};
  const remote={host:'remote',hostname:'Studio Mac',workers:[worker('offline','Offline',[project],2)],projects:{}};
  const document=()=>({machines:Object.fromEntries([local,remote].map(m=>[m.host,{workers:m.workers.map(w=>({id:w.id,intent:w.intent,config:w.config,retiring:w.retiring})),projects:m.projects,workspace:m.workspace}]))});
  await page.route('**/api/fleet/status',r=>r.fulfill({json:{ok:true,machines:[local,remote].map(m=>({...m,state:m===local?'connected':'disconnected',heartbeat:m===local?Date.now()/1000:0,desired_revision:String(revision),applied_revision:String(revision)})),signals:[],conflicts:[]}}));
  await page.route('**/api/fleet/configuration',async r=>{
    if(r.request().method()==='POST'){
      const input=r.request().postDataJSON();requests.push(input);
      if(conflict||input.revision!==String(revision))return r.fulfill({status:409,json:{ok:false,error:'Configuration changed since you opened it. Reload before saving.'}});
      if(input.worker_update){const u=input.worker_update,m=u.host==='local'?local:remote,w=m.workers.find(w=>w.id===u.id);Object.assign(w.config,u.config);w.intent=u.intent;}
      else {
        const u=input.machine_update,m=u.host==='local'?local:remote;
        if(u.action==='add'){const template=m.workers.find(w=>w.id===u.template);m.workers.push({...worker(u.id,'New worker'),config:template?JSON.parse(JSON.stringify(template.config)):{projects:[u.project],concurrency:u.concurrency}});}
        if(u.action==='remove'){const w=m.workers.find(w=>w.id===u.id);w.retiring=true;w.intent='drain';w.config.enabled=false;}
        if(u.action==='project'){const id=u.git.replace('git@github.com:','github.com/').replace(/\.git$/,'');m.projects[id]={git:u.git,path:u.path};m.workspace=u.workspace;}
      }
      revision++;
    }
    return r.fulfill({json:{ok:true,revision:String(revision),document:document(),text:'machines: {}',source:'fleet.yaml'}});
  });
  await page.route('**/api/fleet',r=>{requests.push(r.request().postDataJSON());return r.fulfill({json:{ok:true}});});
  const add=()=>page.locator('button[data-capacity-delta="1"][data-worker="atlas"][data-host="local"]');
  const minus=()=>page.getByRole('button',{name:'Decrease agent limit for Atlas on MacBook Pro',exact:true});
  const scope=()=>page.locator('[data-host="local"] .worker-scope-group').filter({has:page.getByRole('heading',{name:'atlas',exact:true})});
  await page.setViewportSize({width:1440,height:1100});await page.goto('http://127.0.0.1:59688/workers');await add().waitFor();
  check(await page.locator('.machine-controls .worker-stepper').count()===0,'Machines have no worker counter or stepper');
  check(await scope().locator('.worker-count').innerText()==='5','One scheduler with five agents displays an agent limit of five');
  check((await scope().locator('.scope-usage').innerText())==='5 agents running','Project shows the actual running agent count');
  check(!(await scope().locator('summary').first().innerText()).includes('1 worker'),'Project summary does not confuse schedulers with agents');
  const before=JSON.parse(JSON.stringify(local.workers[0]));
  await add().click();await page.waitForFunction(()=>document.querySelector('[data-host="local"] .worker-count').textContent==='6');
  check(local.workers.length===2&&local.workers[0].config.concurrency===6,'Plus increases selected worker concurrency without creating a scheduler');
  check(local.workers[1].config.concurrency===1,'Another project is unchanged');
  check(JSON.stringify({...local.workers[0].config,concurrency:5})===JSON.stringify(before.config),'Provider, tags and checkout settings are preserved');
  check(!(await scope().evaluate(el=>el.open)),'Using plus does not toggle the project disclosure');
  await minus().click();await page.waitForFunction(()=>document.querySelector('[data-host="local"] .worker-count').textContent==='5');
  await minus().click();await page.waitForFunction(()=>document.querySelector('[data-host="local"] .worker-count').textContent==='4');
  check(local.workers[0].active===5&&!requests.some(r=>r.signal),'Reducing capacity below active work does not kill current agents');
  check(await page.getByRole('button',{name:'Decrease agent limit for Tools on MacBook Pro',exact:true}).isDisabled(),'Agent limit cannot fall below one');
  check((await page.locator('[data-host="remote"] .scope-usage').innerText())==='Last known','Offline workers do not report stale counts as live');
  await page.getByRole('button',{name:'Increase agent limit for Offline on Studio Mac',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('[data-host="remote"] .worker-count').textContent==='3');
  check(requests.at(-1).worker_update.host==='remote','Offline edits target the correct machine and worker');
  await scope().locator('summary').first().click();await page.locator('[data-worker="atlas"]>summary').click();
  await page.screenshot({path:'/tmp/hb-worker-controls-visual/activity-desktop.png',fullPage:true});
  for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme:theme});await page.setViewportSize({width,height:900});
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),theme+' '+width+' activity fits');
    check(await add().isVisible(),theme+' '+width+' project control is visible');
    await page.screenshot({path:'/tmp/hb-worker-controls-visual/activity-'+theme+'-'+width+'.png',fullPage:true});
  }
  await page.goto('http://127.0.0.1:59688/workers#view=configuration');
  const settings=page.locator('[data-host="local"] .settings-scope').filter({has:page.getByRole('heading',{name:'atlas',exact:true})});
  await settings.getByRole('heading',{name:'atlas',exact:true}).click();await add().waitFor();
  check(await settings.locator('.worker-count').innerText()==='4','Settings show the same per-worker agent limit');
  await settings.getByRole('button',{name:'Add worker configuration',exact:true}).click();await page.locator('#machine-editor').waitFor({state:'visible'});
  check(await page.locator('#machine-template option').count()===1,'Additional scheduler templates are restricted to the selected project');
  check(await page.locator('#machine-template').inputValue()==='atlas','Additional scheduler inherits the project settings');
  await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  check(requests.at(-1).machine_update.template==='atlas','Explicit scheduler creation uses the selected project template');
  await page.waitForFunction(()=>document.querySelectorAll('[data-host="local"] .settings-scope')[0].querySelectorAll('.worker-record').length===2);
  for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme:theme});await page.setViewportSize({width,height:900});
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),theme+' '+width+' settings fit');
  }
  await page.setViewportSize({width:1440,height:1100});conflict=true;await add().click();
  await page.waitForFunction(()=>document.querySelector('#error').textContent.includes('Configuration changed'));
  check(await add().isEnabled(),'Conflicting saves show an error and leave controls usable');conflict=false;
  await page.locator('[data-host="remote"] .machine-controls [data-machine-action="project"]').click();await page.locator('#machine-editor').waitFor({state:'visible'});
  await page.locator('#machine-git').fill('git@github.com:acme/new-project.git');
  check(await page.locator('#machine-path').inputValue()===''&&(await page.locator('#machine-path').getAttribute('placeholder')).includes('~/Workspace/new-project'),'New projects default to automatic reuse with the clone destination shown');
  check(!await page.locator('#machine-path').evaluate(e=>e.required),'Automatic checkout does not require an explicit path');
  await page.locator('#machine-path').fill('/custom/separate-clone');await page.locator('#machine-workspace').fill('~/Other');
  check(await page.locator('#machine-path').inputValue()==='/custom/separate-clone','Changing workspace preserves a deliberately selected checkout');
  await page.locator('#machine-path').fill('');await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  check(requests.at(-1).machine_update.path==='','Saving automatic checkout sends no forced destination');
  await page.locator('[data-host="remote"] .machine-controls [data-machine-action="project"]').click();await page.locator('#machine-editor').waitFor({state:'visible'});
  check((await page.locator('#machine-editor-status').innerText()).includes('offline'),'Offline project setup explains deferred application');
  await page.locator('#machine-editor-cancel').click();
  await page.getByRole('button',{name:'Add worker for atlas',exact:true}).click();await page.locator('#machine-editor').waitFor({state:'visible'});
  check(!await page.locator('#machine-template').isVisible(),'Adding the first project worker does not offer unrelated machine templates');
  await page.locator('#machine-slots').fill('3');await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  check(requests.at(-1).machine_update.project===project&&requests.at(-1).machine_update.concurrency===3,'New worker setup sends its selected project and agent limit');
  await page.goto('http://127.0.0.1:59688/workers');
  await scope().getByRole('heading',{name:'atlas',exact:true}).click();await add().waitFor();
  check(await scope().locator('[data-capacity-delta="1"]').count()===3,'Projects with multiple configurations expose a limit control for each worker');
  check((await scope().locator('.scope-worker-count').innerText())==='Agent limit: 11','Project total sums its worker limits');
  check(errors.length===0,'No browser exceptions');return {passed:checks.length,checks};
}
