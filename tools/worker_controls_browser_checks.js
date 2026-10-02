// Create /tmp/hb-worker-controls-visual, then run with playwright-cli run-code --filename against serve_auto_workers_fixture.mjs.
async page => {
  const checks=[],errors=[],requests=[];const check=(ok,label)=>{if(!ok)throw Error(label);checks.push(label);};
  page.on('pageerror',e=>errors.push(e.message));
  const project='github.com/acme/atlas';
  const worker=(id,name,pid=20)=>({id,intent:'running',managed:true,pid,active:id==='atlas'?1:0,config:{name,enabled:true,concurrency:2,projects:[project]},runs:[],chiefs:[]});
  let revision=1,conflict=false;
  const local={host:'local',hostname:'MacBook Pro',workers:[worker('atlas','Atlas'),worker('tools','Tools')],projects:{[project]:{git:'git@github.com:acme/atlas.git',path:'~/Workspace/atlas'}}};
  const remote={host:'remote',hostname:'Studio Mac',workers:[],projects:{}};
  const document=()=>({machines:Object.fromEntries([local,remote].map(m=>[m.host,{workers:m.workers.map(w=>({id:w.id,intent:w.intent,config:w.config,retiring:w.retiring})),projects:m.projects,workspace:m.workspace}]))});
  await page.route('**/api/fleet/status',r=>r.fulfill({json:{ok:true,machines:[local,remote].map(m=>({...m,state:m===local?'connected':'disconnected',heartbeat:m===local?Date.now()/1000:0,desired_revision:String(revision),applied_revision:String(revision)})),signals:[],conflicts:[]}}));
  await page.route('**/api/fleet/configuration',async r=>{
    if(r.request().method()==='POST'){
      const input=r.request().postDataJSON();requests.push(input);
      if(conflict||input.revision!==String(revision))return r.fulfill({status:409,json:{ok:false,error:'Configuration changed since you opened it. Reload before saving.'}});
      const u=input.machine_update,m=u.host==='local'?local:remote;
      if(u.action==='add')m.workers.push(worker(u.id,'New worker',null));
      if(u.action==='remove'){const w=m.workers.find(w=>w.id===u.id);w.retiring=true;w.intent='drain';w.config.enabled=false;}
      if(u.action==='project'){const id=u.git.replace('git@github.com:','github.com/').replace(/\.git$/,'');m.projects[id]={git:u.git,path:u.path};m.workspace=u.workspace;}
      revision++;
    }
    return r.fulfill({json:{ok:true,revision:String(revision),document:document(),text:'machines: {}',source:'fleet.yaml'}});
  });
  await page.route('**/api/fleet',r=>{requests.push(r.request().postDataJSON());return r.fulfill({json:{ok:true}});});
  await page.setViewportSize({width:1440,height:1100});await page.goto('http://127.0.0.1:59688/workers');
  await page.locator('.machine-controls').first().waitFor();
  check(await page.locator('[data-host="local"] .worker-count').innerText()==='2','Worker counter excludes no configured workers');
  check(await page.locator('[data-host="remote"] [data-machine-action="remove"]').isDisabled(),'Empty machine cannot remove workers');
  await page.screenshot({path:'/tmp/hb-worker-controls-visual/activity-desktop.png',fullPage:true});
  await page.getByRole('button',{name:'Add a worker on MacBook Pro',exact:true}).click();await page.locator('#machine-editor').waitFor({state:'visible'});
  await page.locator('#machine-template').selectOption('tools');
  check(!await page.locator('#machine-slots').isVisible(),'Copied settings hide unrelated slot input');
  await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  check(requests.at(-1).machine_update.template==='tools','Add worker preserves selected settings');
  await page.getByRole('button',{name:'Remove a worker on MacBook Pro',exact:true}).click();await page.locator('#machine-editor').waitFor({state:'visible'});await page.locator('#machine-worker').selectOption('atlas');
  await page.screenshot({path:'/tmp/hb-worker-controls-visual/remove-desktop.png'});
  await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  check(requests.at(-1).machine_update.action==='remove','Minus requests graceful removal');
  await page.waitForTimeout(600);await page.locator('[data-host="local"] .worker-scope-heading').first().click();await page.locator('[data-worker="atlas"]>summary').click();
  await page.getByRole('button',{name:'Kill now',exact:true}).click();
  check(await page.locator('#fleet-confirm-dialog').isVisible(),'Killing requires an explicit destructive confirmation');
  await page.locator('#fleet-confirm-cancel').click();check(!requests.some(r=>r.signal==='stop'),'Cancelling never sends kill');
  await page.getByRole('button',{name:'Kill now',exact:true}).click();await page.locator('#fleet-confirm-submit').click();await page.waitForTimeout(100);
  check(requests.some(r=>r.signal==='stop'&&r.worker==='atlas'),'Kill targets the retiring worker');
  await page.locator('[data-host="remote"] .machine-controls [data-machine-action="project"]').click();await page.locator('#machine-editor').waitFor({state:'visible'});
  check(await page.locator('#machine-workspace').inputValue()==='~/Workspace','New machine defaults to ~/Workspace');
  check((await page.locator('#machine-editor-status').innerText()).includes('offline'),'Offline project setup explains deferred application');
  await page.locator('#machine-git').fill('git@github.com:acme/my-project.git');
  check(await page.locator('#machine-path').inputValue()==='~/Workspace/my-project','Repository name generates checkout path');
  await page.locator('#machine-workspace').fill('~/Development');
  check(await page.locator('#machine-path').inputValue()==='~/Development/my-project','Workspace changes update proposed checkout');
  await page.locator('#machine-path').fill('~/Development/custom');await page.locator('#machine-git').fill('git@github.com:acme/renamed.git');
  check(await page.locator('#machine-path').inputValue()==='~/Development/custom','Explicit checkout path survives repository editing');
  await page.screenshot({path:'/tmp/hb-worker-controls-visual/project-desktop.png'});
  await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  await page.locator('[data-host="remote"] .machine-controls [data-machine-action="project"]').click();await page.locator('#machine-editor').waitFor({state:'visible'});
  check(await page.locator('#machine-workspace').inputValue()==='~/Development','Workspace default is remembered per machine');await page.locator('#machine-editor-cancel').click();
  await page.goto('http://127.0.0.1:59688/workers#view=configuration');await page.locator('.machine-project').first().waitFor();
  await page.locator('.machine-project').filter({hasText:'~/Development/custom'}).waitFor();
  check((await page.locator('.machine-project').allTextContents()).some(t=>t.includes('~/Development/custom')),'Settings show machine Git repository and checkout path');
  await page.locator('[data-host="local"] .machine-project button').click();await page.locator('#machine-editor').waitFor({state:'visible'});await page.locator('#machine-worker').selectOption('tools');
  check(await page.locator('#machine-git').inputValue()==='git@github.com:acme/atlas.git','Assign reuses machine repository');
  await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  check(requests.at(-1).machine_update.worker==='tools','Project assignment identifies the selected worker');
  await page.screenshot({path:'/tmp/hb-worker-controls-visual/settings-desktop.png',fullPage:true});
  for(const theme of ['light','dark']){
    await page.emulateMedia({colorScheme:theme});
    for(const width of [1440,768,390,320]){
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),theme+' '+width+' settings fit');
      await page.locator('[data-host="local"] .machine-controls [data-machine-action="project"]').click();await page.locator('#machine-editor').waitFor({state:'visible'});await page.locator('#machine-git').fill('git@github.com:acme/a-project-with-a-long-name.git');
      check(await page.evaluate(()=>{const d=document.querySelector('#machine-editor');return d.getBoundingClientRect().width<=innerWidth&&d.scrollWidth<=d.clientWidth;}),theme+' '+width+' dialog fits');
      await page.screenshot({path:'/tmp/hb-worker-controls-visual/project-'+theme+'-'+width+'.png'});
      await page.keyboard.press('Escape');check(!await page.locator('#machine-editor').isVisible(),'Escape closes '+theme+' '+width+' dialog');
    }
  }
  await page.setViewportSize({width:1440,height:1100});
  await page.getByRole('button',{name:'Add a worker on MacBook Pro',exact:true}).click();await page.locator('#machine-editor').waitFor({state:'visible'});conflict=true;await page.locator('#machine-editor-save').click();
  await page.waitForFunction(()=>document.querySelector('#machine-editor-status').textContent.includes('Configuration changed'));
  check((await page.locator('#machine-editor-status').innerText()).includes('Configuration changed'),'Concurrent edits show a conflict without closing or overwriting');
  check(await page.locator('#machine-editor-save').isEnabled(),'Failed saves remain recoverable');await page.locator('#machine-editor-cancel').click();
  check(errors.length===0,'No browser exceptions');return {passed:checks.length,checks};
}
