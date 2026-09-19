// Hold a real older API reply until after a successful profile save.
async page => {
 const checks=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
 await page.reload();await page.waitForFunction(()=>model.csrf&&model.project);
 const origin=await page.evaluate(()=>location.origin);
 const project=await page.evaluate(async()=>{const r=await api({action:'create',title:'Profile ordering control',body:'Preserve content',labels:[]},'Profile response '+crypto.randomUUID());await api({action:'assign_boss',number:1,force:false},r.project.id);return r.project.id});
 await page.goto(origin+'/#project='+encodeURIComponent(project));await page.waitForFunction(project=>model.project?.id===project&&model.signature,project);
 const original=await page.evaluate(async()=>{const r=await api({action:'global_settings'});await api({action:'configure_global',boss_name:'Before delayed reply',if_version:r.version});return r.boss_name});
 let release,heldResolve;const gate=new Promise(r=>release=r),held=new Promise(r=>heldResolve=r);let intercepted=false;
 const handler=async route=>{const data=route.request().postDataJSON();if(!intercepted&&data.operation?.action==='projects'){intercepted=true;const response=await route.fetch();heldResolve();await gate;await route.fulfill({response});}else await route.continue()};
 await page.route('**/api/action',handler);
 try {
  await page.evaluate(()=>{window.olderProfileRead=api({action:'projects',include_hidden:true});return true});await held;
  await page.locator('#self-avatar').click();await page.locator('#global-settings-trigger').click();await page.waitForFunction(()=>!document.querySelector('#global-boss-name').disabled);
  await page.locator('#global-boss-name').fill('After successful save');await page.locator('#global-settings-submit').click();await page.locator('#global-settings-dialog').waitFor({state:'hidden'});
  await page.waitForFunction(()=>model.boss.name==='After successful save');check(true,'Successful rename appears before stale reply');
  release();await page.evaluate(()=>window.olderProfileRead);
  check(await page.evaluate(()=>model.boss.name==='After successful save'),'Older reply cannot revert successful profile save');
  check(await page.locator('#profile-name').textContent()==='After successful save','Profile dropdown retains the saved name');
  const state=await page.evaluate(async()=>({settings:await api({action:'global_settings'}),issue:await api({action:'view',number:1})}));
  check(state.settings.boss_name==='After successful save','Server retains the successful rename');
  check(state.issue.issue.assignee==='human:boss'&&state.issue.issue.version===2,'Rename preserves assignment and issue revision');
  const hostResult=await page.evaluate(async project=>{
   const before={host:model.route.host,boss:model.boss,bossHost:model.bossHost};
   try {
    model.route.host='synthetic-other-store';model.boss={id:'human:boss',name:'Other host profile',version:0};model.bossHost=model.route.host;updateProfile();
    await post('/api/action',{project,operation:{action:'global_settings'},request_id:null,host:null});
    return {name:model.boss.name,badge:document.querySelector('#profile-name').textContent};
   } finally {model.route.host=before.host;model.boss=before.boss;model.bossHost=before.bossHost;updateProfile()}
  },project);
  check(hostResult.name==='Other host profile','Reply from previous host cannot change current profile');
  check(hostResult.badge==='Other host profile','Previous-host reply cannot repaint the profile menu');
  return {passed:checks.length,checks};
 } finally {release();await page.unroute('**/api/action',handler);await page.evaluate(async name=>{const r=await api({action:'global_settings'});await api({action:'configure_global',boss_name:name,if_version:r.version})},original)}
}
