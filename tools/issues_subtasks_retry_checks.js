// Isolated stores: parent CAS and lost child-creation acknowledgments across reload.
async page => {
 const checks=[],ids=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
 await page.reload();await page.waitForFunction(()=>model.csrf&&model.project);
 const origin=await page.evaluate(()=>location.origin),project=await page.evaluate(async()=>{const r=await api({action:'create',title:'Retry parent',body:'Original parent',labels:[]},'Subtask retries '+crypto.randomUUID());return r.project.id});
 const go=async()=>{await page.goto(origin+'/#project='+encodeURIComponent(project)+'&issue=1');await page.waitForFunction(()=>model.detail?.issue.number===1)};
 await go();await page.locator('[data-create-subtask]').click();await page.locator('#editor-subject').fill('Retained child');await page.locator('#editor-body').fill('## Child draft');
 await page.evaluate(async project=>{await api({action:'edit',number:1,title:'Changed parent',body:null,add_labels:[],remove_labels:[],if_version:null},project)},project);
 await page.locator('#editor-submit').click();await page.locator('#editor-conflict:not([hidden])').waitFor();
 check(await page.locator('#editor-body').inputValue()==='## Child draft','Parent conflict preserves child Markdown draft');
 check(await page.locator('#conflict-replace').innerText()==='Create using the latest parent revision','Conflict action explicitly refreshes parent revision');
 await page.locator('#conflict-replace').click();await page.locator('#editor-dialog').waitFor({state:'hidden'});await page.waitForFunction(()=>model.detail?.subtasks?.length===1);
 check(await page.locator('#detail-view h1').innerText().then(t=>t.includes('Changed parent')),'Child creation preserves newer parent content');
 const record=request=>{if(request.url().endsWith('/api/action')){const p=request.postDataJSON();if(p.operation.action==='create_subtask'&&p.operation.title==='One durable child')ids.push(p.request_id)}};page.on('request',record);
 await page.locator('[data-create-subtask]').click();await page.locator('#editor-subject').fill('One durable child');await page.locator('#editor-body').fill('## Durable retry Markdown');
 await page.route('**/api/action',async route=>{const p=route.request().postDataJSON();if(p.operation.action==='create_subtask'&&p.operation.title==='One durable child'){await route.fetch();await route.abort('failed')}else await route.continue()});
 try {
  await page.locator('#editor-submit').click();await page.locator('#editor-error:not([hidden])').waitFor();check(await page.locator('#editor-subject').inputValue()==='One durable child','Lost acknowledgement retains creation draft');
  await page.unroute('**/api/action');await page.reload();await page.waitForFunction(()=>model.detail?.subtasks?.length===2);
  await page.locator('[data-create-subtask]').click();check(await page.locator('#editor-body').inputValue()==='## Durable retry Markdown','Reload restores uncertain child creation draft');
  await page.locator('#editor-submit').click();await page.locator('#editor-dialog').waitFor({state:'hidden'});await page.waitForFunction(()=>model.detail?.subtasks?.length===2);
  check(ids.length===2&&ids[0]&&ids[0]===ids[1],'Reload retry uses original creation request ID');
  const data=await page.evaluate(async project=>await api({action:'subtasks',number:1,include_deleted:true},project),project);
  check(data.issues.filter(i=>i.title==='One durable child').length===1,'Uncertain retry creates exactly one child');
  check(data.issues.find(i=>i.title==='One durable child').body==='## Durable retry Markdown','Retried child preserves Markdown');
  check(await page.evaluate(()=>model.route.issue===1),'Retry finishes on parent');
 } finally {page.removeListener('request',record);await page.unroute('**/api/action')}
 // Stale picker child versions refresh in place and remain retryable.
 await page.evaluate(async project=>await api({action:'create',title:'Picker revision',body:'Before',labels:[]},project),project);
 await page.locator('[data-add-existing-subtask]').click();await page.waitForSelector('[data-existing-subtask="4"]');
 await page.evaluate(async project=>await api({action:'edit',number:4,title:'Updated picker revision',body:null,add_labels:[],remove_labels:[],if_version:null},project),project);
 await page.locator('[data-existing-subtask="4"]').click();await page.locator('#subtask-picker-error:not([hidden])').waitFor();await page.waitForFunction(()=>document.querySelector('[data-existing-subtask="4"]')?.textContent.includes('Updated picker revision'));
 check(await page.locator('#subtask-picker-search').isEnabled(),'Stale picker stays usable');
 await page.locator('[data-existing-subtask="4"]').click();await page.locator('#subtask-picker-dialog').waitFor({state:'hidden'});await page.waitForFunction(()=>model.detail?.subtasks.length===3);
 check(await page.locator('#subtask-list').innerText().then(t=>t.includes('Updated picker revision')),'Stale child picker refreshes before successful retry');
 return {passed:checks.length,checks,project};
}
