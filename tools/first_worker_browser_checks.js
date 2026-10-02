// The complete first-worker journey stays in Activity, using only form controls.
async page => {
  const checks=[],requests=[];const check=(ok,label)=>{if(!ok)throw Error(label);checks.push(label);};
  const id='github.com/poe-internal/poe2';let revision=1;
  const machine={host:'devbox',hostname:'Devbox',state:'connected',workspace:'~/projects',projects:{},workers:[]};
  await page.route('**/api/fleet/status',r=>r.fulfill({json:{ok:true,machines:[{...machine,heartbeat:Date.now()/1000}],signals:[]}}));
  await page.route('**/api/fleet/configuration',r=>{
    if(r.request().method()==='POST'){
      const input=r.request().postDataJSON();requests.push(input);const u=input.machine_update;
      if(u.action==='project')machine.projects[id]={git:u.git,path:'/home/kjopek/poe2',resolved_path:'/home/kjopek/poe2'};
      if(u.action==='add')machine.workers.push({id:u.id,managed:true,intent:'running',pid:null,active:0,config:{projects:[u.project],concurrency:u.concurrency,enabled:true},runs:[]});
      revision++;
    }
    return r.fulfill({json:{ok:true,revision:String(revision),text:'machines: {}',document:{machines:{devbox:machine}}}});
  });
  await page.goto('http://127.0.0.1:59688/workers');await page.reload();
  await page.getByRole('button',{name:'+ Add project',exact:true}).click();
  check(!await page.getByRole('combobox',{name:'Worker',exact:true}).isVisible(),'An empty machine does not offer a dead-end worker assignment selector');
  await page.getByRole('textbox',{name:'Git repository',exact:true}).fill('https://github.com/poe-internal/poe2.git');
  await page.getByRole('button',{name:'Save project',exact:true}).click();await page.getByRole('dialog').waitFor({state:'hidden'});
  const card=page.locator('.activity-machine .machine-project');await card.waitFor();
  check((await card.innerText()).includes('/home/kjopek/poe2'),'Activity shows the reused checkout after project setup');
  check(await card.getByRole('button',{name:'Add worker for poe2',exact:true}).isVisible(),'Activity exposes creation of the first worker');
  check(await card.getByRole('button',{name:'Assign poe2 to a worker',exact:true}).count()===0,'Assignment is not offered when no worker exists');
  check(!await page.getByText('No running workers.',{exact:true}).count(),'An actionable project replaces the empty activity message');
  await page.locator('#worker-save-note').getByRole('button',{name:'Add worker',exact:true}).click();
  await page.getByRole('spinbutton',{name:'Agent limit',exact:true}).waitFor();
  check(await page.getByRole('spinbutton',{name:'Agent limit',exact:true}).inputValue()==='1','The save confirmation opens worker setup directly');
  await page.getByRole('dialog').getByRole('button',{name:'Cancel',exact:true}).click();
  check(machine.workers.length===0,'Cancelling the next step does not silently create a worker');
  for(const width of [1440,390,320]){await page.setViewportSize({width,height:900});check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),width+'px checkout actions fit');}
  await page.screenshot({path:'/tmp/hb-first-worker-setup.png',fullPage:true});
  // The persistent card must survive a reload, independent of the save notice.
  await page.reload();await card.getByRole('button',{name:'Add worker for poe2',exact:true}).click();
  await page.getByRole('spinbutton',{name:'Agent limit',exact:true}).fill('5');
  await page.getByRole('dialog').getByRole('button',{name:'Add worker',exact:true}).click();await page.getByRole('dialog').waitFor({state:'hidden'});
  check(machine.workers.length===1&&machine.workers[0].config.concurrency===5,'The form creates exactly one worker with five agent slots');
  check(requests.at(-1).machine_update.project===id,'The new worker is scoped to the selected project');
  check(await page.locator('.activity-machine .machine-project').count()===0,'The setup card disappears once its worker exists');
  check(await page.locator('.worker-count').innerText()==='5','The requested limit is visible before startup completes');
  machine.workers[0].pid=42;machine.workers[0].active=5;await page.reload();
  await page.getByText('5 agents running',{exact:true}).waitFor();
  check(await page.locator('.worker-count').innerText()==='5','Running activity retains five slots on one worker');
  check(machine.workers.length===1,'Polling and reload never create duplicate workers');
  return {passed:checks.length,checks};
}
