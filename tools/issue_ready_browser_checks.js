// Run with playwright-cli run-code --filename against serve_issue_reopen_fixture.mjs.
async page => {
  const checks=[], errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.setDefaultTimeout(30000);
  page.on('pageerror',e=>errors.push(e.message));
  page.on('dialog',d=>d.accept());
  const base=await page.evaluate(()=>location.origin);
  if(!['http://127.0.0.1:59661','http://127.0.0.1:52061'].includes(base))throw Error('Synthetic fixture required');
  const paired=base.endsWith(':52061'), path=paired?'/issues':'/', surface=paired?'paired':'native';
  await page.route('**/api/inbox',r=>r.fulfill({json:{ok:true,tasks:[],unread_count:0}}));
  if(paired){
    const {code}=await(await page.request.get(base+'/fixture-pairing')).json();
    await page.request.post(base+'/api/pair',{data:{code}});
    for(const name of ['app.js','app.css']){
      const response=await page.request.get('http://127.0.0.1:59661/'+name),body=await response.body();
      await page.route(base+'/issue-web/'+name,r=>r.fulfill({body,contentType:name.endsWith('.js')?'text/javascript':'text/css'}));
    }
  }
  await page.goto(base+path);
  await page.waitForFunction(()=>model.csrf&&model.project&&model.signature);
  const project='named:Ready handoff QA '+Date.now();
  const action=operation=>page.evaluate(({operation,project})=>api(operation,project),{operation,project});
  const view=async n=>(await action({action:'view',number:n})).issue;
  const detail=async n=>{
    await page.goto(base+path+'#project='+encodeURIComponent(project)+'&issue='+n);
    await page.waitForFunction(n=>model.detail?.issue.number===n,n);
  };
  await action({action:'create',title:'Guard Ready handoffs against concurrent claims',body:'Review the usable PR before handing this source to Boss. Ready releases dependent work; CI and deployment are separate.',labels:[]});
  await action({action:'configure_project',prs_enabled:true,subtask_scheduling:'explicit'});
  await action({action:'create',title:'Use the handed-off API',body:'Depends on the guarded source handoff.',labels:[]});
  await action({action:'set_blockers',number:2,blockers:[1],force:false});
  await detail(1);
  let button=page.getByRole('button',{name:'PR ready',exact:true});
  check(await button.isDisabled(),'Missing PR keeps Ready disabled');
  await action({action:'add_pull_request',number:1,url:'https://github.com/example/repo/pull/683'});
  await page.reload();await button.waitFor();
  check(await button.isEnabled(),'Attached PR enables Ready');
  await page.locator('#comment-body').fill('Preserve this unsent review note.');
  let sent, newer;
  const race=async route=>{
    const body=route.request().postDataJSON();
    if(body.operation?.action!=='ready')return route.continue();
    sent=body;
    newer=await action({action:'edit',number:1,add_labels:['concurrent update'],remove_labels:[]});
    await route.continue();
  };
  await page.route('**/api/action',race);
  await button.click();
  await page.getByText(/Ready version guard mismatch/).waitFor();
  check(sent.operation.guard.if_version+1===newer.issue.version,'Browser sends displayed version, not a refreshed mutation guard');
  check(sent.operation.guard.expected_assignee===null&&sent.operation.guard.expected_reservation.length===64,'Browser sends owner and reservation snapshot');
  check((await view(1)).state==='open','Concurrent edit rejects Ready without changing source');
  check((await view(2)).state==='blocked','Rejected Ready keeps dependent blocked');
  check(await button.isEnabled(),'Guard rejection leaves action usable after refresh');
  check(await page.locator('#comment-body').inputValue()==='Preserve this unsent review note.','Guard rejection preserves unsent discussion');
  await page.unroute('**/api/action',race);
  await page.reload();await button.waitFor();
  let first,second;
  const failure=async route=>{
    const body=route.request().postDataJSON();
    if(body.operation?.action!=='ready')return route.continue();
    if(!first){first=body;return route.fulfill({status:503,json:{ok:false,error:{code:'io_error',message:'Synthetic uncertain Ready response'}}});}
    second=body;return route.continue();
  };
  await page.route('**/api/action',failure);
  await button.click();await page.getByText('Synthetic uncertain Ready response',{exact:true}).waitFor();
  await button.focus();await page.keyboard.press('Enter');
  await page.locator('.state-pill.ready').waitFor();
  check(first.request_id===second.request_id,'Uncertain retry reuses stable request ID');
  check(JSON.stringify(first.operation)===JSON.stringify(second.operation),'Uncertain retry preserves exact guards');
  await page.unroute('**/api/action',failure);
  check((await view(2)).state==='open','Successful Ready unblocks dependent');
  const ready=await view(1);
  check(ready.assignee==='human:boss'&&ready.closed_at===null,'Ready assigns Boss without closing');
  check(ready.pull_requests[0].status==='unknown','Ready does not require or invent green CI');
  check((await page.locator('.issue-readiness').innerText()).includes('awaiting PR review'),'Ready status explains review handoff');
  await page.locator('#issue-assignment').selectOption('github');
  await page.waitForFunction(()=>model.detail?.issue.assignment?.kind==='github');
  check((await view(1)).state==='ready','Watcher assignment preserves Ready');
  check((await view(2)).state==='open','Watcher assignment preserves dependent usability');
  check(await page.locator('#issue-assignment').inputValue()==='github','Watcher selection persists');
  await page.reload();await page.locator('.state-pill.ready').waitFor();
  check(await page.locator('#issue-assignment').inputValue()==='github','Watcher survives reload');
  for(const scheme of ['light','dark']){
    await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
    for(const width of [1440,768,390,320]){
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`No overflow ${scheme}/${width}`);
      check(await page.locator('.state-pill.ready').isVisible(),`Ready status visible ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/issue683/${surface}-${scheme}-${width}.png`,fullPage:true});
    }
  }
  await page.setViewportSize({width:390,height:900});
  await action({action:'reopen',number:1});
  await action({action:'block',number:1,force:false});
  await page.reload();await page.locator('.state-pill.blocked').waitFor();
  check(await button.isEnabled(),'Manual hold offers guarded PR handoff');
  let held;
  const hold=async route=>{const body=route.request().postDataJSON();if(body.operation?.action==='ready')held=body;return route.continue();};
  await page.route('**/api/action',hold);
  await button.click();await page.locator('.state-pill.ready').waitFor();
  check(held.operation.clear_manual_hold===true&&held.operation.guard.expected_assignee===null,'Manual hold action sends explicit guarded reconciliation');
  check((await view(1)).manual_blocked===false,'Successful Ready clears manual hold');
  await page.unroute('**/api/action',hold);
  await page.reload();await page.locator('.state-pill.ready').waitFor();
  check((await view(1)).state==='ready','Ready persists on reload');
  check(errors.length===0,'No browser errors: '+errors.join('; '));
  return {surface,checks:checks.length,passed:checks};
}
