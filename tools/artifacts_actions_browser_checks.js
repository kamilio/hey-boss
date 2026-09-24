// Run with playwright-cli run-code --filename against an isolated artifact store.
async page => {
  const checks=[],errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.on('pageerror',e=>errors.push(e.message));
  const base=await page.evaluate(()=>location.origin);
  const queued=await page.locator('html').getAttribute('data-artifact-mobile')==='true';
  const mobile=queued||await page.locator('html').getAttribute('data-issue-mobile')==='true';
  const boot=await(await page.request.get(base+(queued?'/api/artifact-bootstrap':'/api/bootstrap'))).json();
  const project=boot.projects.find(p=>p.name==='Artifact Actions QA').id;
  const action=async operation=>{
    const response=await page.request.post(base+(queued?'/api/artifact-requests':'/api/action'),{headers:queued?{}:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation},request_id:["view","list","links","preview"].includes(operation.command)?null:await page.evaluate(()=>crypto.randomUUID())}});
    let value=await response.json();
    if(queued){const id=value.request.id;for(let n=0;n<100;n++){value=await(await page.request.get(`${base}/api/artifact-requests/${id}`)).json();if(value.request.status!=='pending'){value=value.request.result;break;}await page.waitForTimeout(100);}}
    if(!value.ok)throw Error(JSON.stringify(value));return value;
  };
  const title='Disposable design exploration '+(mobile?'phone':'desktop');
  const created=await action({command:'create',title,body:'# Explorations\n\nKeep the useful work. Archive earlier directions, or delete a scratch document.'});
  const id=created.artifact.id;
  const library=base+'/artifacts#'+await page.evaluate(project=>new URLSearchParams({project}).toString(),project);
  await page.goto(library);await page.reload();await page.locator(`[data-delete="${id}"]`).waitFor();
  const row=()=>page.locator('.artifact-row').filter({has:page.locator(`[data-delete="${id}"]`)});
  await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme:'light'});
  check(await row().locator('a button').count()===0,'Row links and action buttons are separate interactive targets');
  await page.screenshot({path:`output/playwright/issue140/${mobile?'paired':'native'}-desktop.png`});
  await row().locator('[data-archive]').click();
  await page.locator(`[data-delete="${id}"]`).waitFor({state:'detached'});
  check(page.url()===library,'Quick archive does not open the document');
  await page.locator('#artifact-archived').check();await page.locator(`[data-delete="${id}"]`).waitFor();
  check(await row().getByText('Archived',{exact:true}).isVisible(),'Archived filter shows archived documents');
  await row().locator('[data-archive]').click();await page.locator(`[data-delete="${id}"]`).waitFor({state:'detached'});
  await page.locator('#artifact-archived').uncheck();await page.locator(`[data-delete="${id}"]`).waitFor();
  check(true,'Quick restore returns the document to the active library');
  await row().locator('[data-delete]').click();
  const dialog=()=>page.getByRole('dialog');
  check(await dialog().getByRole('button',{name:'Cancel',exact:true}).evaluate(e=>e===document.activeElement),'Delete confirmation focuses Cancel');
  check(await dialog().innerText().then(s=>s.includes(title)&&s.includes('cannot be undone')),'Confirmation names the document and consequences');
  await page.keyboard.press('Escape');await dialog().waitFor({state:'detached'});
  check(await row().locator('[data-delete]').evaluate(e=>e===document.activeElement),'Escape returns focus to the triggering action');
  check((await action({command:'view',id})).artifact.id===id,'Cancelling leaves the document intact');
  for(const width of [390,320]){
    await page.setViewportSize({width,height:844});await page.emulateMedia({colorScheme:'dark'});
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),`${width}px library has no horizontal overflow`);
    check(await row().locator('[data-delete]').evaluate(e=>{const r=e.getBoundingClientRect();return r.width>=44&&r.height>=44&&r.right<=innerWidth;}),`${width}px delete action has a visible 44px touch target`);
    await row().locator('[data-delete]').click();
    check(await dialog().evaluate(e=>{const r=e.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&e.scrollWidth<=e.clientWidth+1;}),`${width}px confirmation fits the screen`);
    await page.screenshot({path:`output/playwright/issue140/${mobile?'paired':'native'}-${width}-delete.png`});
    await dialog().getByRole('button',{name:'Cancel',exact:true}).click();
  }
  // Revision guards must reject stale confirmation without deleting newer work.
  await row().locator('[data-delete]').click();
  await action({command:'edit',id,body:'Concurrent revision',if_version:3});
  await dialog().getByRole('button',{name:'Delete permanently',exact:true}).click();
  await dialog().getByRole('alert').waitFor();
  check((await dialog().getByRole('alert').innerText()).includes('Artifact changed'),'Stale deletion reports a revision conflict');
  check((await action({command:'view',id})).artifact.body==='Concurrent revision','Stale deletion preserves a concurrent revision');
  await dialog().getByRole('button',{name:'Reload latest',exact:true}).click();
  await page.locator(`[data-delete="${id}"]`).waitFor();
  await row().locator('a').click();await page.locator('#artifact-reading').waitFor();
  await page.locator('.artifact-menu summary').click();await page.locator('#artifact-delete').click();
  await dialog().waitFor({state:'visible'});
  await page.keyboard.press('Escape');await dialog().waitFor({state:'detached'});
  check(await page.locator('.artifact-menu summary').evaluate(e=>e===document.activeElement),'Reader confirmation returns focus to the visible menu trigger');
  await page.locator('.artifact-menu summary').click();await page.locator('#artifact-delete').click();
  await dialog().getByRole('button',{name:'Delete permanently',exact:true}).click();
  await page.locator('#artifact-library').waitFor({state:'visible'});await page.locator('#artifact-list[aria-busy]').waitFor({state:'detached'});
  check(await page.locator(`[data-delete="${id}"]`).count()===0,'Reader deletion returns to the library and removes the document');
  check(!(await action({command:'list',archived:true})).artifacts.some(a=>a.id===id),'Permanently deleted documents are absent from the archive');
  if(!mobile){
    const retry=await action({command:'create',title:'Delivery retry scratch',body:'Temporary'}),retryID=retry.artifact.id;
    await page.reload();await page.locator(`[data-delete="${retryID}"]`).waitFor();
    await page.evaluate(()=>{
      const get=Storage.prototype.getItem,set=Storage.prototype.setItem;
      Storage.prototype.getItem=function(key){if(key.endsWith(':action'))throw new DOMException('Storage unavailable','SecurityError');return get.call(this,key);};
      Storage.prototype.setItem=function(key,value){if(key.endsWith(':action'))throw new DOMException('Storage unavailable','SecurityError');return set.call(this,key,value);};
    });
    let originalRequest=null,retriedRequest=null;
    await page.route('**/api/action',async route=>{
      const value=route.request().postDataJSON();
      if(value.operation?.operation?.command!=='delete'){await route.continue();return;}
      if(!originalRequest){originalRequest=value;await route.fetch();await route.abort('failed');}
      else {retriedRequest=value;await route.continue();}
    });
    await page.locator(`[data-delete="${retryID}"]`).click();await dialog().getByRole('button',{name:'Delete permanently',exact:true}).click();
    await dialog().getByRole('button',{name:'Retry deletion',exact:true}).waitFor();
    const retryResponse=page.waitForResponse(r=>r.request().postDataJSON()?.operation?.operation?.command==='delete');
    await dialog().getByRole('button',{name:'Retry deletion',exact:true}).click();await retryResponse;
    check(originalRequest.request_id===retriedRequest.request_id,'Unconfirmed deletion keeps its request identity when browser storage is unavailable');
    check(JSON.stringify(originalRequest.operation)===JSON.stringify(retriedRequest.operation),'Unconfirmed deletion retries the identical revision and payload');
    await dialog().waitFor({state:'detached'});
    await page.unroute('**/api/action');
  }
  check(errors.length===0,'No browser script errors');
  const expected=mobile?20:22;if(checks.length!==expected)throw Error(`Incomplete browser checks: ${checks.length}/${expected}`);
  return {completed:checks.length,expected,checks};
}
