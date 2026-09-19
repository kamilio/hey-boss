// Two synthetic queue hosts sharing an ID must retain separate drafts and caches.
async page => {
 const checks=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
 await page.reload();await page.waitForFunction(()=>model.csrf&&model.project);
 const origin=await page.evaluate(()=>location.origin),project=await page.evaluate(async()=>{const r=await api({action:'create',title:'Local host issue',body:'## Local',labels:[]},'Host isolation '+crypto.randomUUID());return r.project.id}),remote='synthetic-queue';
 const go=async host=>{await page.goto(origin+'/#project='+encodeURIComponent(project)+'&issue=1'+(host?'&host='+host:''));await page.waitForFunction(({project,host})=>model.project?.id===project&&model.detail?.issue.number===1&&model.route.host===(host||model.defaultHost||'')&&model.activeHost===(host||model.defaultHost||'')&&model.detail.issue.title===(host?'Remote host issue':'Local host issue'),{project,host})};
 await go('');await page.locator('#comment-body').fill('Local-only comment');
 check(await page.evaluate(({project,remote})=>detailKey(project,1,'')!==detailKey(project,1,remote),{project,remote}),'Detail cache separates queue hosts');
 check(await page.evaluate(({project,remote})=>draftKey('editor',project,'subtask-1','')!==draftKey('editor',project,'subtask-1',remote),{project,remote}),'Subtask editor drafts separate queue hosts');
 await page.route('**/api/action',async route=>{const p=route.request().postDataJSON();if(p.host===remote){const r=await route.fetch({postData:JSON.stringify({...p,host:null})}),v=await r.json();if(v.issue)v.issue.title='Remote host issue';await route.fulfill({response:r,json:v});}else await route.continue()});
 try {
  await go(remote);check(await page.locator('#comment-body').inputValue()==='','Remote issue never inherits local draft');await page.locator('#comment-body').fill('Remote-only comment');
  await go('');check(await page.locator('#comment-body').inputValue()==='Local-only comment','Returning to local host restores its own draft');
  check(await page.locator('#detail-view h1').innerText().then(t=>t.includes('Local host issue')),'Local issue retains its own cached content');
  await go(remote);check(await page.locator('#comment-body').inputValue()==='Remote-only comment','Remote host restores its own draft');
  check(await page.locator('#detail-view h1').innerText().then(t=>t.includes('Remote host issue')),'Remote issue retains its own content');
 } finally {await page.unroute('**/api/action');await go('')}
 return {passed:checks.length,checks};
}
