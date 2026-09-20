async page => {
 const base='http://127.0.0.1:59642',checks=[],errors=[];const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 page.on('pageerror',e=>errors.push(e.message));
 const {code}=await(await page.request.get(base+'/fixture-pairing')).json();
 const paired=await page.request.post(base+'/api/pair',{data:{code}});check(paired.ok(),'Synthetic phone paired');
 await page.setViewportSize({width:390,height:844});await page.emulateMedia({colorScheme:'light'});
 await page.goto(base+'/agents',{waitUntil:'domcontentloaded'});await page.waitForSelector('.agent-card');
 check(await page.locator('.project-section').count()===3,'Paired web overview receives bridged projects');
 check(!await page.locator('#device-settings').isVisible(),'Phone view keeps service controls secondary');
 check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Paired overview fits phone');
 await page.screenshot({path:'output/playwright/issue42/paired-overview.png',fullPage:true});
 await page.locator('.agent-card').first().click();await page.waitForSelector('.chat-message.assistant');
 check(await page.locator('.chat-message.user').count()===1&&await page.locator('.chat-message.assistant').count()===2,'Full request and replies arrive through authenticated bridge');
 check(await page.locator('.message-body strong').innerText()==='8 reconnect tests passed','Bridged assistant Markdown renders');
 check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Paired conversation fits phone');
 await page.emulateMedia({colorScheme:'dark'});await page.evaluate(()=>scrollTo(0,0));await page.screenshot({path:'output/playwright/issue42/paired-conversation-dark.png',fullPage:true});
 await page.goto(base+'/',{waitUntil:'domcontentloaded'});await page.waitForSelector('nav');
 check(await page.locator('nav a[href="/agents"]').isVisible(),'Agents discoverable from paired app navigation');
 check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Paired app navigation fits phone');
 await page.screenshot({path:'output/playwright/issue42/paired-app-navigation.png',fullPage:true});
 check(errors.length===0,'No paired web JavaScript errors');return {passed:checks.length,checks};
}
