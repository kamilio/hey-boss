// Run against tools/serve_agents_fixture.mjs through playwright-cli run-code.
async page => {
 await page.unroute('**/api/fleet/conversation?*');await page.unroute('**/api/fleet/status');
 const checks=[],errors=[];const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 const base='http://127.0.0.1:59641';const shots='output/playwright/issue42/';
 page.on('pageerror',e=>errors.push(e.message));
 const fits=()=>page.evaluate(()=>document.documentElement.scrollWidth<=document.documentElement.clientWidth);
 await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme:'light'});
 await page.goto(base+'/agents',{waitUntil:'domcontentloaded'});await page.waitForSelector('.agent-card');
 check(await page.locator('.project-section').count()===3,'Tasks grouped into three projects');
 check(await page.locator('.project-section').first().locator('.agent-card:not(.history-card)').count()===2,'Agents on separate devices appear together under their project');
 check(await page.locator('.location-label').allTextContents().then(v=>v.includes('Devbox')),'Device is a simple label');
 check(!(await page.locator('#overview-page').innerText()).match(/Supervisor|worker|capacity|session ID|heartbeat/),'Overview uses human language');
 check(await page.locator('.is-quiet').innerText()==='Last seen','Disconnected task never says it is live');
 await page.locator('.project-history summary').click();check(await page.locator('.history-card').isVisible(),'Completed conversation is accessible');
 await page.locator('.project-history summary').focus();await page.locator('#refresh').click();await page.waitForTimeout(600);
 check(await page.locator('.project-history').evaluate(e=>e.open),'Refresh preserves history expansion');
 await page.screenshot({path:shots+'overview-desktop.png',fullPage:true});
 await page.goto(base+'/agents#project=named%3AAtlas',{waitUntil:'domcontentloaded'});await page.waitForSelector('.agent-card');
 check(await page.locator('.project-section').count()===1,'Project filter shows only selected project');
 await page.locator('#show-all').click();await page.waitForSelector('.agent-card');check(await page.locator('.project-section').count()===3,'All projects restores overview');
 await page.locator('.agent-card').first().focus();await page.keyboard.press('Enter');await page.waitForSelector('.chat-message.assistant');
 check(page.url().includes('/agents/session#'),'Agent opens on a separate detail page');
 check(await page.locator('.chat-message.user').count()===1,'Original request included');
 check(await page.locator('.chat-message.assistant').count()===2,'Complete saved replies included');
 check(await page.locator('.chat-message.tool').count()===2,'Tool calls and results included');
 check(await page.locator('.message-body strong').innerText()==='8 reconnect tests passed','Assistant Markdown renders');
 await page.locator('.chat-message.tool summary').first().click();check(await page.locator('.chat-message.tool pre').first().isVisible(),'Tool detail expands');
 await page.locator('.chat-message.tool summary').first().focus();await page.keyboard.press('Enter');check(!await page.locator('.chat-message.tool pre').first().isVisible(),'Tool detail works with keyboard');
 await page.evaluate(()=>scrollTo(0,0));await page.screenshot({path:shots+'conversation-desktop.png',fullPage:true});check(await fits(),'Desktop conversation fits');
 for(const width of [390,320]){
  await page.setViewportSize({width,height:844});await page.evaluate(()=>scrollTo(0,0));check(await fits(),width+'px conversation has no horizontal overflow');
  check(await page.locator('#session-title').evaluate(e=>e.getBoundingClientRect().right<=innerWidth),width+'px heading fits');
  await page.screenshot({path:shots+'conversation-'+width+'.png',fullPage:true});
 }
 await page.emulateMedia({colorScheme:'dark'});await page.setViewportSize({width:1440,height:1000});await page.evaluate(()=>scrollTo(0,0));await page.screenshot({path:shots+'conversation-dark.png',fullPage:true});
 check(await page.evaluate(()=>getComputedStyle(document.documentElement).colorScheme)==='dark','Dark theme uses dark surfaces');
 await page.locator('#back').click();await page.waitForSelector('.agent-card');check(page.url().includes('project=named%3AAtlas'),'Back retains project context');
 await page.setViewportSize({width:390,height:844});await page.screenshot({path:shots+'overview-mobile-dark.png',fullPage:true});check(await fits(),'Mobile overview fits');
 // Long history and live updates: read earlier text without being moved.
 let calls=0;const items=Array.from({length:24},(_,i)=>({id:String(i),role:i===0?'user':'assistant',text:'Message '+i+'\n\n'+('A saved conversation remains readable while the work continues. '.repeat(6))}));
 let extra=[];
 await page.route('**/api/fleet/conversation?*',route=>{const cursor=Number(/cursor=(\d+)/.exec(route.request().url())?.[1]||0);calls++;const messages=cursor===0?items:extra;extra=[];return route.fulfill({json:{ok:true,messages,cursor:cursor===0?24:cursor+messages.length,has_more:false,availability:'available'}});});
 await page.locator('.agent-card').first().click();await page.waitForFunction(()=>document.querySelectorAll('.chat-message').length===24);
 await page.evaluate(()=>scrollTo(0,350));await page.waitForTimeout(150);const before=await page.evaluate(()=>scrollY);
 extra=[{id:'24',role:'assistant',text:'A fresh update has arrived.'}];
 await page.evaluate(()=>document.getElementById('refresh').click());await page.waitForFunction(()=>document.querySelectorAll('.chat-message').length===25);
 check(Math.abs(await page.evaluate(()=>scrollY)-before)<5,'Live append preserves position while reading earlier messages');
 check(await page.locator('#jump-live').isVisible(),'Latest activity button appears while reading history');
 await page.locator('#jump-live').click();await page.waitForFunction(()=>document.getElementById('conversation-end').getBoundingClientRect().bottom<=innerHeight+5);check(!await page.locator('#jump-live').isVisible(),'Latest activity returns to the end');
 await page.locator('#refresh').click();await page.waitForTimeout(500);check(await page.locator('.chat-message').count()===25,'Polling does not duplicate history');
 // Pagination keeps earlier messages, rather than replacing them.
 await page.unroute('**/api/fleet/conversation?*');
 await page.route('**/api/fleet/conversation?*',route=>{const older=route.request().url().includes('before=');return route.fulfill({json:{ok:true,messages:[{id:older?'0':'1',role:'assistant',text:older?'First page':'Second page'}],cursor:2,older_cursor:older?0:1,has_earlier:!older,has_more:false,availability:'available'}});});
 await page.reload({waitUntil:'domcontentloaded'});await page.waitForSelector('#load-earlier:not([hidden])');check((await page.locator('#conversation').innerText()).includes('Second page'),'Latest history opens first');await page.locator('#load-earlier').click();await page.waitForFunction(()=>document.querySelectorAll('.chat-message').length===2);check((await page.locator('#conversation').innerText()).includes('First page')&&(await page.locator('#conversation').innerText()).includes('Second page'),'Pagination retains both pages');check((await page.locator('#conversation').innerText()).indexOf('First page')<(await page.locator('#conversation').innerText()).indexOf('Second page'),'Earlier history prepends in chronological order');
 await page.unroute('**/api/fleet/conversation?*');
 await page.route('**/api/fleet/conversation?*',route=>route.fulfill({status:503,json:{ok:false,error:'This device is disconnected. Reconnect to load its conversation.'}}));
 await page.reload();await page.waitForSelector('#error:not([hidden])');check((await page.locator('#error').innerText()).includes('disconnected'),'Failed remote load gives an actionable error');
 await page.unroute('**/api/fleet/conversation?*');
 await page.route('**/api/fleet/status',route=>route.fulfill({json:{ok:true,machines:[],signals:[],conflicts:[]}}));
 await page.goto(base+'/agents',{waitUntil:'domcontentloaded'});await page.waitForSelector('.agents-empty');check(await fits(),'Empty state fits');await page.screenshot({path:shots+'empty-mobile.png',fullPage:true});
 await page.unroute('**/api/fleet/status');
 check(errors.length===0,'No JavaScript errors');
 return {passed:checks.length,checks,requests:calls};
}
