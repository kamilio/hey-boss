// Run against serve_issue_timeline_fixture.mjs using playwright-cli run-code.
async page => {
 const checks=[],errors=[];page.on('pageerror',e=>errors.push(e.message));
 const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
 await page.goto('about:blank');
 const project='named%3ATimeline%20QA',suffix='#project='+project+'&issue=1';
 const mobile='http://127.0.0.1:52947';
 const {code}=await(await page.request.get(mobile+'/fixture/pairing')).json();
 check((await page.request.post(mobile+'/api/pair',{data:{code}})).ok(),'Paired phone authenticated');
 for(const [mode,base] of [['desktop','http://127.0.0.1:48948/'],['phone',mobile+'/issues']]){
  await page.goto(base+suffix);await page.locator('.issue-activity').first().waitFor();
  check(await page.locator('#history-toggle').count()===0,mode+': activity visible by default');
  check(await page.locator('#comments .comment-card').count()===2,mode+': comments appear once');
  const text=await page.locator('#comments').innerText();
  check(text.includes('Codex · gpt-6-astra')&&text.includes('GitHub PR watcher')&&text.includes('Boss'),mode+': human, model and watcher attribution');
  check(text.includes('added bug needs-review; removed enhancement'),mode+': readable label differences');
  check(await page.locator('#comments img').count()===0,mode+': event metadata escaped');
  check(await page.locator('#comments .comment-body strong').first().textContent()==='Tests pass.',mode+': Markdown comments remain rich');
  const order=await page.locator('#comments > [data-event-id], #comments > [data-comment-id]').evaluateAll(nodes=>nodes.map(n=>n.textContent));
  check(order.findIndex(s=>s.includes('Tests pass.'))<order.findIndex(s=>s.includes('added bug')),mode+': comment before subsequent label change');
  for(const theme of ['light','dark'])for(const width of [1440,768,390,320]){
   await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
   await page.evaluate(theme=>{document.documentElement.dataset.theme=theme;},theme);
   check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${mode}/${theme}/${width}: no horizontal overflow`);
   check(await page.locator('.activity-content').evaluateAll(nodes=>nodes.every(n=>n.getBoundingClientRect().right<=innerWidth)),`${mode}/${theme}/${width}: activity stays within viewport`);
   if([1440,390].includes(width)){
    await page.locator('#comments .comment-card').first().scrollIntoViewIfNeeded();
    await page.screenshot({path:`/tmp/hb-timeline-visual-review/${mode}-${theme}-${width}.png`});
   }
  }
  await page.setViewportSize({width:1024,height:900});
  await page.locator('#comment-body').fill('Unsent draft survives reading earlier activity');
  const ids=await page.locator('#comments > [data-event-id]').evaluateAll(nodes=>nodes.map(n=>n.dataset.eventId));
  await page.getByRole('button',{name:'Load earlier activity'}).click();
  await page.locator('.history-more').waitFor({state:'hidden'});
  const loaded=await page.locator('#comments > [data-event-id]').evaluateAll(nodes=>nodes.map(n=>n.dataset.eventId));
  check(loaded.length>ids.length&&new Set(loaded).size===loaded.length,mode+': earlier page prepends without duplicates');
  check(await page.locator('#comment-body').inputValue()==='Unsent draft survives reading earlier activity',mode+': pagination preserves draft');
  const card=page.locator('#comments .comment-card').first();
  if(await card.locator('.comment-resolve').innerText()==='Unresolve')await card.locator('.comment-resolve').click();
  await card.locator('.comment-resolve').click();await page.locator('#comments .resolved-comment').first().waitFor();
  check(await page.locator('#comment-body').inputValue()==='Unsent draft survives reading earlier activity',mode+': resolving preserves draft');
  await page.locator('#comments .resolved-comment summary').first().focus();await page.keyboard.press('Enter');
  check(await page.locator('#comments .resolved-comment details').first().evaluate(n=>n.open),mode+': keyboard expands resolved comment');
  await page.goto(base+'#project='+project+'&issue=2');await page.locator('.issue-activity').first().waitFor();
  check(await page.locator('#comments .comment-card').count()===0,mode+': issue without comments shows its opening event');
  await page.goto(base+'#project='+project+'&issue=3');await page.locator('.issue-activity').first().waitFor();
  check(await page.locator('#comment-form').count()===0,mode+': deleted issue keeps read-only activity');
 }
 // A failed page keeps existing content and can be retried.
 await page.route('**/api/action',async route=>{const body=route.request().postDataJSON();if(body.operation?.action==='timeline')return route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{message:'Temporary failure'}})});return route.continue();});
 await page.goto('http://127.0.0.1:48948/'+suffix);await page.locator('.timeline-error').waitFor();
 check(await page.locator('#comments .comment-card').count()===2,'Initial failure preserves comments');
 await page.unroute('**/api/action');await page.locator('.timeline-error button').click();await page.locator('.issue-activity').first().waitFor();
 check(await page.locator('.timeline-error').count()===0,'Retry restores activity');
 let held, arrived;
 const pending=new Promise(resolve=>{arrived=resolve;});
 await page.route('**/api/action',async route=>{const body=route.request().postDataJSON();if(body.operation?.action==='timeline'&&body.operation.number===1){held=route;arrived();return;}await route.continue();});
 await page.goto('about:blank');
 await page.goto('http://127.0.0.1:48948/'+suffix);await pending;
 await page.goto('http://127.0.0.1:48948/#project='+project+'&issue=2');await page.locator('.issue-activity').first().waitFor();
 await held.fulfill({status:200,contentType:'application/json',body:JSON.stringify({ok:true,entries:[{kind:'event',id:999,created_at:1}],events:[{id:999,actor:'human:STALE',action:'closed',created_at:1,data:{}}],comments:[],next_before:null})}).catch(()=>{});
 await page.waitForTimeout(100);
 check(!(await page.locator('#comments').innerText()).includes('STALE'),'Late response cannot replace a different issue');
 await page.unroute('**/api/action');
 check(errors.length===0,'No browser JavaScript errors: '+errors.join('; '));
 return {count:checks.length,checks};
}
