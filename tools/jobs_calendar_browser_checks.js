// Run through playwright-cli against serve_jobs_ui_fixture.mjs. Temporary PNGs go under /tmp.
async page=>{
 const origin=await page.evaluate(()=>location.origin),paired=origin.endsWith('52049'),surface=paired?'paired':'desktop';
 if(!['http://127.0.0.1:59649','http://127.0.0.1:52049'].includes(origin))throw Error('Isolated fixture required');
 const project='named:Jobs UI QA',checks=[],errors=[],queries=[];
 const check=(condition,name)=>{if(!condition)throw Error(name);checks.push(name);};
 await page.unroute('**/api/action');page.setDefaultTimeout(20000);page.on('pageerror',e=>errors.push(e.message));page.on('dialog',d=>d.accept());
 await page.route('**/api/inbox',r=>r.fulfill({json:{ok:true,tasks:[]}}));
 await page.route('**/api/jobs/runtime',r=>r.fulfill({json:{ok:true,machine:'fixture',machines:[],catalog:{models:[]}}}));
 if(paired){const {code}=await(await page.request.get(origin+'/fixture-pairing')).json();check((await page.request.post(origin+'/api/pair',{data:{code}})).ok(),'Pair mobile');}
 const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
 const api=async operation=>{const response=await page.request.post(origin+'/api/action',{headers:paired?{}:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'job',operation},...(['create','edit','set_enabled','delete','run_now','stop'].includes(operation.command)?{request_id:"calendar-"+Date.now()+"-"+Math.random().toString(16).slice(2)}:{})}});const v=await response.json();if(!v.ok)throw Error(JSON.stringify(v));return v;};
 const markdown=await(await page.request.get('http://127.0.0.1:52049/fixture-instructions.md')).text();
 const prefix=surface+'-'+Date.now();
 const create=async(id,name,cron,timezone,enabled=true)=>api({command:'create',id:prefix+id,definition:{name,cron,timezone,harness:'codex',model:'exact-model'},markdown,enabled});
 const daily=(await create('-daily','Morning release review','0 8 * * *','America/Chicago')).job;
 const hourly=(await create('-hourly','Hourly build check','0 * * * *','UTC')).job;
 await create('-tokyo','Tokyo handoff','0 18 * * 1-5','Asia/Tokyo');
 const states=['pending','running','succeeded','failed','cancelled','skipped'],runs=[];
 for(let index=0;index<states.length;index++){
  const state=states[index],id=prefix+'-'+state;
  await create('-'+state,{pending:'Waiting for runtime',running:'Release inspection',succeeded:'Dependency review',failed:'Nightly build review',cancelled:'Manual release check',skipped:'Overlapping run'}[state],'0 8 * * *','America/Chicago',false);
  let run=(await api({command:'run_now',id})).run;
  if(state==='skipped'){run=(await api({command:'run_now',id})).run;}else if(!['pending','running'].includes(state))await api({command:'stop',id,run_id:run.id});
  const at=Date.now()-((7-index)*600000);
  check((await page.request.post('http://127.0.0.1:52049/fixture-calendar-run',{data:{run:run.id,state,at}})).ok(),'Seed '+state+' history');
  if(state==='cancelled')check((await page.request.post('http://127.0.0.1:52049/fixture-session',{data:{run:run.id}})).ok(),'Seed saved session');
  runs.push({id,run:run.id,state});
 }
 const watch=req=>{if(req.url().endsWith('/api/action')){const op=req.postDataJSON()?.operation?.operation;if(op)queries.push(op);}};page.on('request',watch);
 const ready=()=>page.waitForFunction(()=>!document.querySelector('#jobs-calendar')?.hidden&&document.querySelector('#calendar-grid')?.getAttribute('aria-busy')==='false'&&document.querySelector('.calendar-day'));
 await page.setViewportSize({width:1440,height:1100});
 await page.goto(origin+'/jobs?qa='+prefix+'#'+'project='+encodeURIComponent(project)+'&view=calendar');await ready();
 check(await page.locator('.jobs-layout').isHidden(),'Calendar is a dedicated view');
 check((await page.locator('#calendar-zone').inputValue()).length>0,'Display timezone explicit');
 check(await page.locator('.calendar-day').count()<=42,'Visible month bounded');
 await page.locator('#calendar-zone').selectOption('America/Chicago');await ready();
 for(const mode of ['month','week','agenda']){
  await page.locator(`[data-mode="${mode}"]`).click();await ready();
  for(const width of [1440,768,390])for(const colorScheme of ['light','dark']){
   await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme});
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${mode} ${width} ${colorScheme}: no overflow`);
   if(width!==768)await page.screenshot({path:`/tmp/hb-calendar-qa/${surface}-${mode}-${width}-${colorScheme}.png`,fullPage:true});
  }
 }
 await page.setViewportSize({width:1440,height:1100});await page.emulateMedia({colorScheme:'light'});
 await page.locator('[data-mode="month"]').click();await ready();
 await page.locator('.calendar-day h3 button').first().focus();await page.keyboard.press('ArrowRight');check(await page.locator('.calendar-day h3 button').nth(1).evaluate(e=>e===document.activeElement),'Arrow key day navigation');
 await page.keyboard.press('Enter');await page.locator('#calendar-dialog').waitFor({state:'visible'});await page.keyboard.press('Escape');check(await page.locator('#calendar-dialog').isHidden(),'Escape closes accessible dialog');
 for(const record of runs){
  await page.locator('#calendar-job').selectOption(record.id);await ready();
  const event=page.locator('#calendar-grid .calendar-event.'+record.state).first();await event.click();
  await page.waitForFunction(()=>document.querySelector('.calendar-facts'));
  const text=await page.locator('#calendar-detail').textContent();
  check(text.includes('Schedule timezone')&&text.includes('Scheduled time')&&text.includes('Actual start')&&text.includes('Actual finish'),'Separate schedule and actual '+record.state);
  if(record.state==='pending')check(text.includes('runtime is unavailable')||text.includes('No connected job service'),'Pending reason visible');
  if(record.state==='skipped')check(text.includes('overlap')&&text.includes('Not run'),'Skipped reason and no actual start');
  if(record.state==='cancelled'){
   const href=await page.locator('#calendar-detail a[href^="/agents/session"]').getAttribute('href');
   await page.screenshot({path:`/tmp/hb-calendar-qa/${surface}-run-detail.png`});
   const newPage=await page.context().newPage();await newPage.goto(origin+href);await newPage.waitForFunction(()=>document.body.textContent.includes('Synthetic saved job result.'));await newPage.close();
   check(true,'Saved session opens from calendar');
  }
  await page.keyboard.press('Escape');
 }
 await page.locator('#calendar-job').selectOption(daily.id);await ready();
 await page.locator('#calendar-grid .calendar-event.scheduled').first().click();await page.waitForFunction(()=>document.querySelector('#job-name'));
 check(await page.locator('#job-name').inputValue()==='Morning release review','Upcoming opens editor');
 await page.getByRole('button',{name:'Pause schedule',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#job-enabled-toggle')?.textContent==='Resume schedule');
 await page.locator('#jobs-view-calendar').click();await ready();check(await page.locator('.calendar-event.scheduled').count()===0,'Pause clears projections');
 const saved=(await api({command:'view',id:daily.id})).job;
 const edited=(await api({command:'edit',id:daily.id,if_revision:saved.revision,definition:{...saved.snapshot.definition,name:'Edited morning review',cron:'0 10 * * *'},markdown:null})).job;
 await api({command:'set_enabled',id:daily.id,if_revision:edited.revision,enabled:true});
 await page.locator('#calendar-refresh').click();await ready();check((await page.locator('#calendar-grid').textContent()).includes('Edited morning review'),'External edit and resume refresh future');
 const current=(await api({command:'view',id:daily.id})).job;await api({command:'delete',id:daily.id,if_revision:current.revision});await page.locator('#calendar-refresh').click();await ready();check(await page.locator('.calendar-event.scheduled').count()===0,'Deletion clears projections');
 const historyJob=(await api({command:'view',id:runs[4].id})).job;await api({command:'delete',id:historyJob.id,if_revision:historyJob.revision});await page.locator('#calendar-refresh').click();await ready();await page.locator('#calendar-job').selectOption(historyJob.id);await ready();check(await page.locator('.calendar-event.cancelled').count()===1,'Deleted job history retained');
 await page.locator('#calendar-job').selectOption(hourly.id);await ready();
 await page.locator('#calendar-next').click();await ready();await page.locator('.calendar-overflow').first().click();await page.waitForFunction(()=>document.querySelector('.calendar-day-list .calendar-event'));
 check(await page.locator('.calendar-day-list .calendar-event').count()>=23,'Dense hourly day expands all entries');await page.keyboard.press('Escape');
 const dense=(await create('-dense','Minute density test','* * * * *','UTC')).job;
 await page.locator('#calendar-refresh').click();await ready();await page.locator('#calendar-job').selectOption(dense.id);await ready();await page.locator('.calendar-overflow').first().click();await page.waitForFunction(()=>document.querySelectorAll('.calendar-day-list .calendar-event').length===100);
 await page.locator('#calendar-more').click();await page.waitForFunction(()=>document.querySelectorAll('.calendar-day-list .calendar-event').length===200);
 check(true,'Dense day pages 100 entries');await page.keyboard.press('Escape');
 let fail=true;
 const failure=async route=>{if(route.request().postDataJSON()?.operation?.operation?.command==='calendar'&&fail)return route.fulfill({json:{ok:false,error:{message:'Synthetic offline failure'}}});return route.continue();};
 await page.route('**/api/action',failure);await page.locator('#calendar-refresh').click();await page.waitForFunction(()=>document.querySelector('[data-calendar-retry]'));
 check((await page.locator('#calendar-status').textContent()).includes('Synthetic offline'),'Error state is actionable');await page.screenshot({path:`/tmp/hb-calendar-qa/${surface}-error.png`});fail=false;await page.locator('[data-calendar-retry]').click();await ready();await page.unroute('**/api/action',failure);
 const delay=async route=>{if(route.request().postDataJSON()?.operation?.operation?.command==='calendar')await page.waitForTimeout(1000);return route.continue();};
 await page.route('**/api/action',delay);await page.locator('#calendar-refresh').click();check((await page.locator('#calendar-status').textContent()).includes('Loading'),'Loading state');await page.screenshot({path:`/tmp/hb-calendar-qa/${surface}-loading.png`});await ready();await page.unroute('**/api/action',delay);
 check(queries.filter(q=>q.command==='calendar').every(q=>q.days<=42),'All summary requests bounded to visible range');check(queries.filter(q=>q.command==='calendar_entries').every(q=>q.days===1),'Detail requests limited to one day');check(errors.length===0,'No browser JS errors');
 page.off('request',watch);
 return {surface,checks:checks.length,calendarRequests:queries.filter(q=>q.command==='calendar').length};
}
