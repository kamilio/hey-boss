// Run with playwright-cli run-code --filename against an isolated worker-config server.
// Screenshots are temporary and must be removed after visual inspection.
async page => {
  const base=page.url().split('/').slice(0,3).join('/'), checks=[], errors=[];
  await page.unroute('**/api/fleet/status');await page.unroute('**/api/fleet/conversation?*');await page.unroute('**/api/fleet/steer');
  const config=await page.request.get(base+'/api/fleet/configuration').then(r=>r.json());
  const bootstrap=await page.request.get(base+'/api/bootstrap').then(r=>r.json());
  await page.request.post(base+'/api/fleet/configuration',{headers:{'X-Hey-Boss-CSRF':bootstrap.csrf},data:{worker_update:{host:'local',id:'tools',intent:'pause',config:{provider:'codex'}},revision:config.revision,save:true}});
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.on('pageerror',e=>errors.push(e.message));
  const fits=()=>page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth && [...document.querySelectorAll('dialog[open]')].every(d=>d.getBoundingClientRect().left>=0&&d.getBoundingClientRect().right<=innerWidth));
  await page.setViewportSize({width:1440,height:1000});
  page.once('dialog',dialog=>dialog.accept());
  await page.goto(base+'/workers?visual='+Date.now()+'#view=configuration');
  await page.locator('.worker-scope-heading').first().click();
  await page.locator('[data-edit-worker="tools"]').click();
  await page.locator('#worker-editor').waitFor({state:'visible'});
  await page.waitForFunction(()=>!document.getElementById('worker-provider').disabled);
  check(await page.locator('#worker-provider').inputValue()==='codex','Existing workers default to Codex');
  await page.locator('#worker-provider').focus();
  check(await page.locator('#worker-provider').evaluate(el=>document.activeElement===el),'Provider selector is keyboard focusable');
  await page.locator('#worker-provider').selectOption('claude');
  await page.locator('#worker-editor-preview').click();
  await page.waitForFunction(()=>!document.getElementById('worker-editor-save').disabled);
  check((await page.locator('#worker-editor-changes').innerText()).includes('codex → claude'),'Review identifies provider change');
  await page.locator('#worker-editor-save').click();await page.locator('#worker-editor').waitFor({state:'hidden'});
  check((await page.request.get(base+'/api/fleet/configuration').then(r=>r.json())).document.machines.local.workers[0].config.provider==='claude','Provider persists through real configuration API');
  await page.locator('[data-edit-worker="tools"]').click();
  await page.locator('#worker-editor').waitFor({state:'visible'});
  await page.waitForFunction(()=>!document.getElementById('worker-provider').disabled);
  check(await page.locator('#worker-provider').inputValue()==='claude','Reopening preserves saved provider');
  await page.locator('#worker-provider').selectOption('pi');await page.locator('#worker-editor-preview').click();
  await page.waitForFunction(()=>!document.getElementById('worker-editor-save').disabled);
  for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme:theme});await page.setViewportSize({width,height:width<500?844:1000});
    check(await fits(),`Provider editor fits ${width}px ${theme}`);
    await page.screenshot({path:`/tmp/hb-provider-work/settings-${theme}-${width}.png`,fullPage:true});
  }
  await page.locator('#worker-editor-save').click();await page.locator('#worker-editor').waitFor({state:'hidden'});
  check((await page.request.get(base+'/api/fleet/configuration').then(r=>r.json())).document.machines.local.workers[0].config.provider==='pi','Pi saves without altering worker intent');
  let provider='claude',instruction;
  const run=()=>({id:provider+'-run',project_id:'named:Atlas',project_name:'Atlas',number:1,title:'Verify agent controls',actor_id:provider+':owned',session_id:'12345678-1234-1234-1234-123456789abc',state:'running',started_at:Date.now()-60000,finished_at:null});
  await page.route('**/api/fleet/status',route=>route.fulfill({json:{ok:true,machines:[{host:'local',hostname:'Test Mac',state:'connected',heartbeat:Date.now()/1000,workers:[{id:'tools',pid:123,config:{provider,enabled:true},runs:[run()]}]}]}}));
  await page.route('**/api/fleet/conversation?*',route=>route.fulfill({json:{ok:true,availability:'available',cursor:3,has_more:false,run:run(),messages:[{id:'1',role:'user',label:'You',text:'Verify the proof file.'},{id:'2',role:'tool',label:'Read',text:'VERIFIED_GOAL'},{id:'3',role:'assistant',label:provider==='claude'?'Claude':'Pi',text:'The proof file is verified.'}]}}));
  await page.route('**/api/fleet/steer',route=>{instruction=route.request().postDataJSON();return route.fulfill({json:{ok:true,state:'queued',request_id:instruction.request_id}});});
  for(provider of ['claude','pi']){
    await page.goto(base+'/agents/session#host=local&run='+provider+'-run&project=named%3AAtlas');
    await page.locator('.chat-message.assistant').waitFor();
    check((await page.locator('.message-author').allTextContents()).includes(provider==='claude'?'Claude':'Pi'),`${provider} conversation has correct author`);
    check((await page.locator('#session-context').innerText()).includes(provider==='claude'?'Claude':'Pi'),`${provider} task has correct provider`);
    await page.locator('#steer-open').click();
    await page.locator('#steer-text').fill('Verify the additional requirement.');
    check(await fits(),`${provider} steering dialog fits mobile`);
    await page.screenshot({path:`/tmp/hb-provider-work/steering-${provider}-mobile.png`,fullPage:true});
    await page.locator('#steer-send').click();await page.locator('#steer-dialog').waitFor({state:'hidden'});
    check(instruction?.run===provider+'-run'&&instruction?.text==='Verify the additional requirement.',`${provider} sends steering to exact run`);
    await page.locator('.chat-message.tool summary').click();
    check(await page.locator('.chat-message.tool pre').isVisible(),`${provider} tool details expand`);
    check(await fits(),`${provider} conversation fits mobile`);
    await page.screenshot({path:`/tmp/hb-provider-work/conversation-${provider}-mobile.png`,fullPage:true});
  }
  check(errors.length===0,'No JavaScript exceptions');
  return {passed:checks.length,checks};
}
