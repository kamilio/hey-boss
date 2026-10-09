// Run against the isolated native/paired issue fixture; never production data.
async page => {
  const checks=[],errors=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const base=await page.evaluate(()=>location.origin);
  if(!['http://127.0.0.1:59639','http://127.0.0.1:52039'].includes(base))throw Error('Isolated fixture required');
  const paired=base.endsWith(':52039'),path=paired?'/issues':'/',surface=paired?'paired':'native';
  page.setDefaultTimeout(20000);
  page.on('pageerror',error=>errors.push(error.message));
  await page.route('**/api/inbox',r=>r.fulfill({json:{ok:true,tasks:[],unread_count:0}}));
  await page.route('**/fixture-instructions.md',r=>r.fulfill({path:'src/jobs/fixtures/original.md',contentType:'text/markdown'}));
  for(const name of ['app.js','components.js'])await page.route(base+(paired?'/issue-web/':'/')+name,r=>r.fulfill({path:'src/issues/web/'+name,contentType:'text/javascript'}));
  if(paired){const {code}=await(await page.request.get(base+'/fixture-pairing')).json();check((await page.request.post(base+'/api/pair',{data:{code}})).ok(),'Paired browser authenticated');}
  const project='named:Quick action QA';
  await page.goto(base+path+'?qa='+Date.now()+'#project='+encodeURIComponent(project));
  await page.waitForFunction(()=>typeof model!=='undefined'&&model.csrf&&model.project);
  const records=await(await page.request.get('http://127.0.0.1:52039/fixture-runs.json')).json();
  for(const width of [1440,390]){
    await page.setViewportSize({width,height:width===390?844:1000});
    for(const record of records){
      await page.goto(base+path+'#project='+encodeURIComponent(project)+'&issue='+record.number);
      await page.waitForFunction(n=>model.detail?.issue.number===n,record.number);
      const readiness=page.locator('.issue-readiness');
      check(await readiness.getByRole('heading',{name:'Scheduled job'}).count()===1,`${surface} ${width} ${record.state}: dedicated ownership label`);
      check(await readiness.locator('.readiness-status').textContent()===record.state[0].toUpperCase()+record.state.slice(1),`${surface} ${width} ${record.state}: truthful result`);
      check(!/Ready for agents|Move to draft/.test(await readiness.innerText()),`${surface} ${width} ${record.state}: no ordinary-pickup guidance`);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${surface} ${width} ${record.state}: no horizontal overflow`);
      check((await page.locator('body').innerText()).includes('Unicode: żółw — 日本語.'),`${surface} ${width} ${record.state}: detail rendered`);
      await readiness.scrollIntoViewIfNeeded();
      await page.screenshot({path:`/tmp/hb-scheduled-jobs-qa/${surface}-${width}-${record.state}.png`,fullPage:true});
    }
    await page.goto(base+path+'#project='+encodeURIComponent(project));
    await page.waitForFunction(()=>model.issues.length>0&&!model.detail);
    check(await page.getByText('Scheduled job',{exact:true}).count()>0,`${surface} ${width}: readable actor labels`);
    await page.screenshot({path:`/tmp/hb-scheduled-jobs-qa/${surface}-${width}-list.png`,fullPage:true});
  }
  check(errors.length===0,'No browser JavaScript errors');
  return {surface,checks:checks.length,records};
}
