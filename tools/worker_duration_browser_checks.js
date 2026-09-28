// Run against tools/serve_auto_workers_fixture.mjs with playwright-cli run-code --filename.
async page => {
  const checks=[];
  const check=(ok,label)=>{if(!ok)throw Error(label);checks.push(label);};
  await page.setViewportSize({width:1440,height:950});
  await page.goto('http://127.0.0.1:59688/workers');
  const group=page.locator('[data-host="local"] .worker-scope-group').first();
  const groupTime=group.locator(':scope > summary .agent-runtime');
  check(await groupTime.isVisible(),'Agent runtime is visible while the group is collapsed');
  const snapshot=await (await page.request.get('http://127.0.0.1:59688/api/fleet/status')).json();
  const start=snapshot.machines[0].workers.find(w=>w.id==='poe-code').runs[0].started_at;
  const shown=/^(\d+)m (\d{2})s$/.exec(await groupTime.innerText());
  check(shown&&Math.abs(Number(shown[1])*60+Number(shown[2])-Math.floor((Date.now()-start)/1000))<=1,'Runtime starts from the agent start time');
  await group.locator('summary').first().click();
  const worker=page.locator('[data-worker="poe-code"]');
  const workerTime=worker.locator(':scope > summary .agent-runtime');
  check(await workerTime.isVisible(),'Worker runtime is visible without opening task details');
  check(await workerTime.innerText()===await groupTime.innerText(),'Group and worker refer to the same displayed agent');
  const before=await workerTime.innerText();
  await page.waitForTimeout(1400);
  check(await workerTime.innerText()!==before,'Runtime ticks without waiting for the next fleet poll');
  check(await page.locator('[data-worker="tools"] .agent-runtime').count()===0,'Idle workers do not show a running duration');
  await worker.locator('summary').first().click();
  check(await worker.locator('.activity-task .agent-runtime:visible').count()===2,'Each expanded agent has its own duration');
  for(const scheme of ['light','dark']){
    await page.emulateMedia({colorScheme:scheme});
    for(const width of [1440,390,320]){
      await page.setViewportSize({width,height:950});
      check(await groupTime.isVisible()&&await workerTime.isVisible(),scheme+' '+width+'px keeps duration visible');
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),scheme+' '+width+'px fits without scrolling');
      check(await workerTime.evaluate(el=>el.scrollWidth<=el.clientWidth),scheme+' '+width+'px does not clip duration');
      await page.screenshot({path:'output/playwright/worker-duration/'+scheme+'-'+width+'.png',fullPage:true});
    }
  }
  return {passed:checks.length,checks};
}
