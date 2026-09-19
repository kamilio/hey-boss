// Simulate leaving and restoring a cached page with an Inbox response held.
async page => {
  const checks=[],errors=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
  page.on('dialog',dialog=>dialog.accept().catch(()=>{}));
  page.on('pageerror',error=>errors.push(error.message));
  await page.reload();await page.waitForFunction(()=>model.csrf&&model.project);
  const origin=await page.evaluate(()=>location.origin);
  const project=await page.evaluate(async()=>(await api({action:'create',title:'Page lifecycle fixture',body:'',labels:[]},'Page lifecycle '+crypto.randomUUID())).project.id);
  await page.goto(origin+'/#project='+encodeURIComponent(project)+'&issue=1');
  await page.waitForFunction(project=>model.project?.id===project&&model.detail?.issue.number===1,project);
  await page.locator('#comment-body').fill('Draft survives cached-page restoration');
  await page.waitForFunction(()=>inboxLoading===null);
  let release,received,finished,cancelled=0;
  const gate=new Promise(resolve=>release=resolve),started=new Promise(resolve=>received=resolve),done=new Promise(resolve=>finished=resolve);
  const onFailure=request=>{if(request.url().endsWith('/api/inbox'))cancelled++};
  page.on('requestfailed',onFailure);
  await page.route('**/api/inbox',async route=>{
    received();await gate;
    try{await route.fulfill({json:{ok:true,tasks:[],unread:0}})}catch{/* Browser disposed of the cancelled request. */}finally{finished()}
  });
  try{
    await page.evaluate(()=>{inboxAt=0;refreshInboxBadge()});await started;
    check(true,'Background Inbox request is actually pending');
    await page.evaluate(()=>dispatchEvent(new PageTransitionEvent('pagehide')));
    await page.waitForTimeout(100);
    check(cancelled>0,'Leaving the page cancels its pending request');
  }finally{
    release();await done;await page.unroute('**/api/inbox');page.removeListener('requestfailed',onFailure);
    await page.evaluate(()=>dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true})));
  }
  await page.waitForFunction(()=>!document.querySelector('#connection').classList.contains('offline')&&inboxLoading===null);
  check(await page.locator('#comment-body').inputValue()==='Draft survives cached-page restoration','Cached-page restoration preserves the comment draft');
  const response=await page.evaluate(()=>api({action:'view',number:1}));
  check(response.ok&&response.issue.title==='Page lifecycle fixture','Restored page can retrieve its issue again');
  await page.locator('[data-back]').click();await page.keyboard.press('/');
  check(await page.locator('#issue-search').evaluate(el=>el===document.activeElement),'Restored page navigation and search shortcut work');
  check(errors.length===0,'No page lifecycle runtime errors');
  return {passed:checks.length,checks};
}
