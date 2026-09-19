// Linking a native notice elsewhere must refresh the open issue without changing it.
async page => {
  page.removeAllListeners('dialog');page.on('dialog',d=>d.accept().catch(()=>{}));
  const origin=await page.evaluate(()=>location.origin),checks=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
  await page.goto(origin+'/?related-notice='+Date.now());
  await page.waitForFunction(()=>model.csrf&&model.project);
  const project=await page.evaluate(async()=>(await api({action:'create',title:'Notice refresh QA',body:'Preserve this issue',labels:[]},'Notice refresh QA '+Date.now())).project.id);
  const bootstrap=await(await page.request.get(origin+'/api/bootstrap')).json();
  const inbox=async fields=>{const r=await page.request.post(origin+'/api/inbox',{headers:{'X-Hey-Boss-CSRF':bootstrap.csrf},data:fields});if(!r.ok())throw Error(await r.text());return r.json()};
  const original=(await inbox({action:'view',task_id:'notice-1'})).task;
  await page.goto(origin+'/?related-notice='+Date.now()+'#project='+encodeURIComponent(project)+'&issue=1');
  await page.waitForFunction(()=>model.detail?.issue.number===1&&document.querySelector('#related-notices')?.textContent.includes('No linked notices'));
  const before=await page.evaluate(()=>JSON.stringify(model.detail.issue));
  await page.locator('#comment-body').fill('Unsent update survives linking');
  try {
    await inbox({action:'link',task_id:'notice-1',issue:{project,number:1,host:null}});
    await page.locator('#related-notices .related-notice-link').waitFor({timeout:12000});
    check(true,'External notice link appears automatically');
    check(await page.locator('#comment-body').evaluate(el=>el===document.activeElement),'Notice refresh preserves comment focus');
    check(await page.locator('#comment-body').inputValue()==='Unsent update survives linking','Notice refresh preserves comment draft');
    check(await page.evaluate(before=>JSON.stringify(model.detail.issue)===before,before),'Linking changes no issue content or revision');
    check((await inbox({action:'view',task_id:'notice-1'})).task.status===original.status,'Linking changes no notice status');
    const relatedLink=page.locator('#related-notices .related-notice-link');await relatedLink.focus();
    await page.evaluate(async()=>{await inboxSnapshot(true);await refreshInboxBadge()});
    check(await relatedLink.evaluate(el=>el===document.activeElement),'Unchanged notice polling preserves link focus');
    await page.locator('#comment-body').focus();
    await inbox({action:'link',task_id:'notice-1',issue:null});
    await page.waitForFunction(()=>document.querySelector('#related-notices')?.textContent.includes('No linked notices'),null,{timeout:12000});
    check(true,'External unlink removes backlink automatically');
    check(await page.locator('#comment-body').inputValue()==='Unsent update survives linking','Unlinking preserves draft');
    check(await page.locator('#comment-body').evaluate(el=>el===document.activeElement),'Unlinking preserves focus');
    await inbox({action:'link',task_id:'notice-1',issue:{project,number:1,host:null}});
    await relatedLink.waitFor({timeout:12000});await relatedLink.focus();
    await inbox({action:'link',task_id:'notice-1',issue:null});
    await page.waitForFunction(()=>document.querySelector('#related-notices')?.textContent.includes('No linked notices'),null,{timeout:12000});
    check(await page.locator('#related-notices h2').evaluate(el=>el===document.activeElement&&el.checkVisibility()),'Removed notice link returns focus to its visible section heading');
    await page.route('**/api/inbox',async route=>{if(route.request().postDataJSON().action==='list')await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{code:'unavailable',message:'Synthetic disconnect'}})});else await route.continue()});
    await page.reload();await page.getByText('Notices unavailable',{exact:true}).waitFor();
    check(await page.locator('#related-notices').isVisible(),'Temporarily unavailable notices retain their section');
    await page.unroute('**/api/inbox');await page.evaluate(()=>refreshInboxBadge());
    await page.getByText('No linked notices',{exact:true}).waitFor();
    check(await page.locator('#related-notices').isVisible(),'Related notices recover without reopening issue');
    check(await page.locator('#comment-body').inputValue()==='Unsent update survives linking','Native reconnect preserves saved comment draft');
  } finally {
    await page.unroute('**/api/inbox');
    await inbox({action:'link',task_id:'notice-1',issue:original.issue||null});
    await page.locator('#comment-body').fill('');await page.evaluate(()=>saveComment());
  }
  return {passed:checks.length,checks};
}
