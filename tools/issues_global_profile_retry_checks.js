// Lost save acknowledgements must retry one global name change.
async page => {
  page.removeAllListeners('dialog');page.on('dialog',d=>d.accept().catch(()=>{}));
  const origin=await page.evaluate(()=>location.origin),checks=[],requests=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
  await page.goto(origin+'/?profile-retry='+Date.now());await page.waitForFunction(()=>model.csrf&&model.project);
  const project=await page.evaluate(async()=>{const value=await api({action:'create',title:'Profile retry QA',body:'Preserve',labels:[]},'Profile retry QA '+Date.now());await api({action:'assign_boss',number:1,force:false},value.project.id);return value.project.id});
  await page.goto(origin+'/?profile-retry='+Date.now()+'#project='+encodeURIComponent(project));await page.waitForFunction(()=>model.signature&&model.issues.length===1);
  const before=await page.evaluate(async()=>({settings:await api({action:'global_settings'}),issue:JSON.stringify({version:model.issues[0].version,assignee:model.issues[0].assignee,title:model.issues[0].title,body:model.issues[0].body,labels:model.issues[0].labels})})),name='Retry profile '+Date.now();
  const record=request=>{if(request.url().endsWith('/api/action')){const body=request.postDataJSON();if(body.operation.action==='configure_global'&&body.operation.boss_name===name)requests.push(body.request_id)}};page.on('request',record);
  await page.locator('#self-avatar').click();await page.locator('#global-settings-trigger').click();await page.waitForFunction(()=>!document.querySelector('#global-boss-name').disabled);await page.locator('#global-boss-name').fill(name);
  await page.route('**/api/action',async route=>{const body=route.request().postDataJSON();if(body.operation.action==='configure_global'&&body.operation.boss_name===name){await route.fetch();await route.abort('failed')}else await route.continue()});
  try {
    await page.locator('#global-settings-submit').click();await page.locator('#global-settings-error:not([hidden])').waitFor();
    check(await page.locator('#global-boss-name').inputValue()===name,'Lost acknowledgement preserves name draft');
    check(await page.locator('#global-settings-submit').isEnabled(),'Failed save can retry');
    await page.unroute('**/api/action');await page.locator('#global-settings-submit').click();await page.locator('#global-settings-dialog').waitFor({state:'hidden'});
    await page.waitForFunction(name=>model.boss.name===name&&model.signature,name);
    check(requests.length===2&&requests[0]&&requests[0]===requests[1],'Retry uses the identical mutation ID');
    const current=await page.evaluate(async()=>({settings:await api({action:'global_settings'}),issue:JSON.stringify({version:model.issues[0].version,assignee:model.issues[0].assignee,title:model.issues[0].title,body:model.issues[0].body,labels:model.issues[0].labels})}));
    check(current.settings.version===before.settings.version+1,'Retry increments global version only once');
    check(current.issue===before.issue,'Retry preserves assignment and issue revision');
    check(await page.locator('#profile-name').innerText()===name,'Saved global name appears in profile menu');
  } finally {
    page.removeListener('request',record);await page.unroute('**/api/action');
    await page.evaluate(async name=>{const settings=await api({action:'global_settings'});await api({action:'configure_global',boss_name:name,if_version:settings.version})},before.settings.boss_name);
    if(await page.locator('#global-settings-dialog').evaluate(el=>el.open))await page.keyboard.press('Escape');
  }
  return {passed:checks.length,checks};
}
