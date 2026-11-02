// Run against tools/serve_auto_workers_fixture.mjs with playwright-cli run-code --filename.
async page => {
  const checks=[];
  const check=(ok,label)=>{if(!ok)throw Error(label);checks.push(label);};
  await page.setViewportSize({width:1440,height:950});
  await page.goto('http://127.0.0.1:59688/workers');
  const worker=page.locator('.project-worker[data-worker="poe-code"]');
  const workerTime=worker.locator('.project-worker-task .agent-runtime').first();
  await workerTime.waitFor();
  const snapshot=await (await page.request.get('http://127.0.0.1:59688/api/fleet/status')).json();
  const start=snapshot.machines[0].workers.find(w=>w.id==='poe-code').runs[0].started_at;
  const shown=/^(\d+)m (\d{2})s$/.exec(await workerTime.innerText());
  check(shown&&Math.abs(Number(shown[1])*60+Number(shown[2])-Math.floor((Date.now()-start)/1000))<=1,'Runtime starts from the agent start time');
  const before=await workerTime.innerText();
  await page.waitForTimeout(1400);
  check(await workerTime.innerText()!==before,'Runtime ticks without waiting for the next fleet poll');
  check(await page.locator('[data-worker="tools"] .agent-runtime').count()===0,'Idle workers do not show a running duration');
  check(await worker.locator('.project-worker-task .agent-runtime:visible').count()===2,'Each expanded agent has its own duration');
  for(const scheme of ['light','dark']){
    await page.emulateMedia({colorScheme:scheme});
    for(const width of [1440,390,320]){
      await page.setViewportSize({width,height:950});
      check(await workerTime.isVisible(),scheme+' '+width+'px keeps duration visible');
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),scheme+' '+width+'px fits without scrolling');
      check(await workerTime.evaluate(el=>el.scrollWidth<=el.clientWidth),scheme+' '+width+'px does not clip duration');
      await page.screenshot({path:'output/playwright/worker-duration/'+scheme+'-'+width+'.png',fullPage:true});
    }
  }
  return {passed:checks.length,checks};
}
