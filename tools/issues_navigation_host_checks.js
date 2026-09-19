// A cancelled host transition must never copy the visible comment to that host.
async page => {
  const checks=[],errors=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
  page.on('dialog',dialog=>dialog.accept().catch(()=>{}));
  page.on('pageerror',error=>errors.push(error.message));
  await page.reload();await page.waitForFunction(()=>model.csrf&&model.project);
  const origin=await page.evaluate(()=>location.origin),remote='navigation-remote';
  const project=await page.evaluate(async()=>(await api({action:'create',title:'Host navigation fixture',body:'',labels:[]},'Host navigation '+crypto.randomUUID())).project.id);
  await page.goto(origin+'/#project='+encodeURIComponent(project)+'&issue=1');
  await page.waitForFunction(project=>model.project?.id===project&&model.detail?.issue.number===1,project);
  await page.locator('#comment-body').fill('Local comment must stay local');
  let release,received,handled;
  const gate=new Promise(resolve=>release=resolve),started=new Promise(resolve=>received=resolve),done=new Promise(resolve=>handled=resolve);
  await page.route('**/api/action',async route=>{
    const payload=route.request().postDataJSON();
    if(payload.host===remote&&(payload.operation?.action||payload.action)==='projects'){
      received();await gate;
      try{const response=await route.fetch({postData:JSON.stringify({...payload,host:null})});await route.fulfill({response});}finally{handled()}
    }else await route.continue();
  });
  try{
    await page.evaluate(remote=>navigate({host:remote}),remote);await started;
    check(await page.locator('#comment-body').inputValue()==='Local comment must stay local','Pending host transition retains visible draft');
    await page.evaluate(()=>navigate({host:model.defaultHost||''}));
    await page.waitForFunction(()=>model.detail?.issue.number===1);
    check(await page.evaluate(({project,remote})=>storage.get(draftKey('comment',project,1,remote))===null,{project,remote}),'Cancelling host transition never writes local draft into remote storage');
    check(await page.locator('#comment-body').inputValue()==='Local comment must stay local','Cancelling host transition restores local comment');
  }finally{release();await done;await page.unroute('**/api/action')}
  await page.waitForTimeout(100);
  check(await page.evaluate(()=>model.route.host===(model.defaultHost||'')&&model.activeHost===(model.defaultHost||'')),'Late host reply cannot change current route');
  check(await page.locator('#comment-body').inputValue()==='Local comment must stay local','Late host reply cannot change local draft');
  check(errors.length===0,'No host navigation runtime errors');
  return {passed:checks.length,checks};
}
