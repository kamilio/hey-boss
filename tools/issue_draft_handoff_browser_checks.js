// Playwright CLI run-code --filename, against serve_issue_reopen_fixture.mjs.
async page => {
  const checks=[],errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.setDefaultTimeout(30000);
  page.on('pageerror',e=>errors.push(e.message));
  const base=await page.evaluate(()=>location.origin);
  if(!['http://127.0.0.1:59661','http://127.0.0.1:52061'].includes(base))throw Error('Synthetic fixture required');
  const paired=base.endsWith(':52061'),path=paired?'/issues':'/',surface=paired?'paired':'native';
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
  const project='named:Draft handoff QA '+Date.now();
  const action=operation=>page.evaluate(({operation,project})=>api(operation,project),{operation,project});
  const view=async n=>(await action({action:'view',number:n})).issue;
  await action({action:'create',title:'Published API source with follow-up repair',body:'Useful for dependent development. Keep this source in draft; review, CI and repair work remain separate.',labels:[],draft:true});
  await action({action:'configure_project',prs_enabled:true,subtask_scheduling:'explicit'});
  await action({action:'create',title:'Build on the published API',body:'Runnable after a development handoff.',labels:[]});
  await action({action:'set_blockers',number:2,blockers:[1],force:false});
  await page.goto(base+path+'#project='+encodeURIComponent(project)+'&issue=1');
  await page.waitForFunction(()=>model.detail?.issue.number===1);
  const button=page.getByRole('button',{name:'Development handoff',exact:true});
  check(await button.isDisabled(),'Draft handoff requires an attached PR');
  check(await page.getByRole('button',{name:'Mark ready',exact:true}).isVisible(),'Ordinary undraft action remains separate');
  await action({action:'add_pull_request',number:1,url:'https://github.com/example/repo/pull/684'});
  await page.reload();await button.waitFor();
  check(await button.isEnabled(),'Published draft offers development handoff');
  await page.locator('#comment-body').fill('Unsent review findings');
  let sent;
  const race=async route=>{
    const body=route.request().postDataJSON();
    if(body.operation?.action!=='ready')return route.continue();
    sent=body;
    await action({action:'edit',number:1,add_labels:['new evidence'],remove_labels:[]});
    await route.continue();
  };
  await page.route('**/api/action',race);
  await button.click();
  await page.getByText(/Ready version guard mismatch/).waitFor();
  check(sent.operation.keep_draft===true&&sent.operation.guard.expected_assignee===null,'Action explicitly preserves draft with displayed owner guard');
  check((await view(1)).draft&&(await view(1)).state==='open','Concurrent edit preserves draft lifecycle');
  check((await view(2)).state==='blocked','Rejected handoff leaves dependent blocked');
  check(await page.locator('#comment-body').inputValue()==='Unsent review findings','Rejection preserves unsent text');
  await page.unroute('**/api/action',race);
  await page.reload();await button.waitFor();
  await button.focus();await page.keyboard.press('Enter');
  await page.getByRole('heading',{name:'Development handoff recorded'}).waitFor();
  const source=await view(1);
  check(source.state==='ready'&&source.draft&&source.assignee==='human:boss','Handoff retains draft and assigns Boss');
  check(source.closed_at===null&&source.pull_requests[0].status==='unknown','Handoff asserts neither completion nor verified CI');
  check((await view(2)).state==='open','Eligible dependent is released');
  check(await page.getByRole('button',{name:'Mark ready',exact:true}).count()===0,'Recorded handoff offers no accidental undraft shortcut');
  check((await page.locator('.issue-readiness').innerText()).includes('No worker pickup'),'Readiness explains source scheduling');
  for(const scheme of ['light','dark']){
    await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
    for(const width of [1440,768,390,320]){
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`No horizontal overflow ${scheme}/${width}`);
      check(await page.getByRole('heading',{name:'Development handoff recorded'}).isVisible(),`Handoff notice visible ${scheme}/${width}`);
      await page.screenshot({path:`output/playwright/issue684/${surface}-${scheme}-${width}.png`,fullPage:true});
    }
  }
  await page.getByRole('button',{name:'Reopen issue',exact:true}).click();
  await page.getByRole('heading',{name:'This issue is a draft'}).waitFor();
  check((await view(1)).draft&&(await view(1)).state==='open','Reopening withdraws handoff without enabling source pickup');
  check((await view(2)).state==='blocked','Withdrawing handoff blocks new dependent pickups');
  await page.reload();await page.getByRole('heading',{name:'This issue is a draft'}).waitFor();
  check(errors.length===0,'No browser errors: '+errors.join('; '));
  return {surface,checks:checks.length,passed:checks};
}
