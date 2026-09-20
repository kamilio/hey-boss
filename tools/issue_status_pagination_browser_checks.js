async page => {
 const checks=[],errors=[];
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 await page.unrouteAll({behavior:'wait'});
 page.on('pageerror',e=>errors.push(e.message));
 const fixture='http://127.0.0.1:52070';
 const {code}=await (await page.request.get(fixture+'/fixture/pairing')).json();
 check((await page.request.post(fixture+'/api/pair',{data:{code}})).ok(),'Paired fixture');
 for(const mode of ['desktop','paired']){
  await page.setViewportSize({width:390,height:1000});
  await page.emulateMedia({colorScheme:mode==='desktop'?'light':'dark',reducedMotion:'reduce'});
  const baseline=await (await page.request.get(fixture+'/fixture/history')).json();
  const expected=baseline.updates.map(update=>update.comment);
  const url=mode==='desktop'?`http://127.0.0.1:4796/?history-qa=${Date.now()}#project=named%3AProgress%20Studio&issue=1`:fixture+`/project-resource?history-qa=${Date.now()}#project=named%3AProgress%20Studio&issue=1`;
  const historyRequests=[];
  const track=request=>{
   if(request.method()!=='POST')return;
   const body=request.postDataJSON();
   if(body?.operation?.action==='status_history')historyRequests.push(body.operation);
  };
  page.on('request',track);
  await page.goto(url,{waitUntil:'commit'});
  await page.locator('.progress-history summary').click();
  await page.locator('.progress-history-list li').nth(19).waitFor();
  check(historyRequests.length===1&&historyRequests[0].before===null,`${mode}: first page starts a fresh snapshot`);
  const incoming=await page.request.post(fixture+'/fixture/status');
  check(incoming.ok(),`${mode}: new updates arrive between pages`);
  const newest=(await incoming.json()).issue.status.comment;
  const refresh=page.getByRole('button',{name:'Refresh history',exact:true});
  await refresh.waitFor({state:'visible',timeout:22000});
  const more=page.getByRole('button',{name:'Show older updates',exact:true});
  let loaded=20;
  while(loaded<expected.length){
   await more.click();loaded=Math.min(loaded+20,expected.length);
   await page.locator('.progress-history-list li').nth(loaded-1).waitFor();
  }
  const actual=await page.locator('.progress-history-list li p').allTextContents();
  check(JSON.stringify(actual)===JSON.stringify(expected),`${mode}: every older update appears once in snapshot order`);
  check(historyRequests.length===Math.ceil(expected.length/20)&&historyRequests.slice(1).every(request=>request.before===baseline.snapshot_at),`${mode}: every older page carries the reading boundary`);
  check(!await page.getByRole('button',{name:'Show older updates',exact:true}).isVisible(),`${mode}: snapshot pagination finishes`);
  check(await refresh.isVisible(),`${mode}: older pages retain the new-update notice`);
  // A manual refresh starts at the latest update, while current progress stays live.
  await refresh.focus();await page.keyboard.press('Enter');
  await page.waitForFunction(newest=>document.querySelector('.progress-history-list li p')?.textContent===newest,newest);
  check(historyRequests.at(-1).before===null,`${mode}: refresh resets the snapshot`);
  check(await page.locator('.progress-history-list').evaluate(list=>document.activeElement===list&&list.scrollTop===0),`${mode}: refresh keeps keyboard focus at the newest entries`);
  check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${mode}: updated history fits the phone viewport`);
  await page.screenshot({path:`output/playwright/issue70/pagination-${mode}.png`});
  page.off('request',track);
 }
 check(errors.length===0,`No script errors: ${errors}`);
 return {checks:checks.length,errors};
}
