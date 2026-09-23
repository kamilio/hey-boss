// Use with playwright-cli run-code against the isolated Agents fixture.
async page => {
 const base='http://127.0.0.1:59641', shots='output/playwright/issue120';
 const checks=[],errors=[];page.on('pageerror',e=>errors.push(e.message));
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 await page.goto(base+'/agents#project=named%3AAtlas');
 await page.waitForSelector('.chief-panel');
 check((await page.locator('.chief-panel').innerText()).includes('Waiting · 15 minutes left'),'Next pass countdown');
 check((await page.locator('.chief-owner').first().innerText()).includes('Atlas worker'),'Owning worker visible');
 await page.locator('.chief-conversation').click();
 await page.waitForSelector('.chat-message');
 check((await page.locator('#session-title').innerText()).includes('Chief'),'Chief saved conversation loads through real history reader');
 check(!await page.locator('#session-issue').isVisible(),'Chief has no fabricated issue link');
 check(!await page.locator('#takeover-open').isVisible()&&!await page.locator('#steer-open').isVisible(),'Chief has no issue-only controls');
 await page.goto(base+'/agents#project=named%3AAtlas');
 await page.waitForSelector('.chief-panel');
 for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
   await page.setViewportSize({width,height:950});
   await page.emulateMedia({colorScheme:theme});
   await page.screenshot({path:shots+`/chief-${theme}-${width}.png`,fullPage:true});
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme} fits ${width}px`);
 }
 await page.setViewportSize({width:390,height:844});
 await page.locator('.chief-conversation').focus();
 await page.locator('#refresh').click();
 await page.locator('.chief-conversation').focus();
 await page.waitForTimeout(3500);
 check(await page.locator('.chief-conversation').evaluate(e=>e===document.activeElement),'Polling preserves keyboard focus');
 const snapshot=await(await page.request.get(base+'/api/fleet/status')).json();
 const chief=snapshot.machines[0].workers[0].chiefs[0];
 for(const [name,state,enabled,connected,pid] of [['running','running',true,true,123],['paused','idle',false,true,123],['offline','idle',true,false,123],['stopped','idle',true,true,null],['failed','failed',true,true,123]]){
   const data=JSON.parse(JSON.stringify(snapshot)),m=data.machines[0],w=m.workers[0];m.state=connected?'connected':'disconnected';m.heartbeat=Date.now()/1000;w.pid=pid;w.config.enabled=enabled;w.chiefs[0]={...chief,state,finished_at:state==='running'?null:chief.finished_at};
   await page.route('**/api/fleet/status',r=>r.fulfill({json:data}));await page.locator('#refresh').click();
   const expected={running:'Running',paused:'Paused',offline:'Device disconnected',stopped:'Worker stopped',failed:'Needs attention'}[name];
   await page.waitForFunction(expected=>document.querySelector('.chief-panel').textContent.includes(expected),expected);
   check(true,name+' state visible');await page.screenshot({path:shots+'/chief-'+name+'.png',fullPage:true});await page.unroute('**/api/fleet/status');
 }
 check(errors.length===0,'No browser errors');return {passed:checks.length,checks};
}
