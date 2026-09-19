async page => {
 const checks=[],errors=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
 page.on('pageerror',e=>errors.push(e.message));
 await page.goto('http://127.0.0.1:4782/');await page.waitForFunction(()=>model.csrf && model.project);await page.setViewportSize({width:1440,height:1000});
 const project=await page.evaluate(async()=>{
   const name='Issue order QA '+Date.now();let project;
   for(let n=1;n<=6;n++){const value=await api({action:'create',title:'Queue issue '+n,body:'# Requirements\nPreserve markdown and PRs.',labels:[n%2?'ready':'backlog']},name);project=value.project.id;}
   await api({action:'add_pull_request',number:3,url:'https://github.com/example/repo/pull/31'},project);return project;
 });
 await page.goto('http://127.0.0.1:4782/#project='+encodeURIComponent(project));await page.waitForSelector('.issue-order-handle');
 const order=()=>page.locator('.issue-row').evaluateAll(rows=>rows.map(row=>Number(row.dataset.issueNumber)));
 const expect=async(expected,name)=>{await page.waitForFunction(expected=>JSON.stringify([...document.querySelectorAll('.issue-row')].map(row=>Number(row.dataset.issueNumber)))===JSON.stringify(expected)&&!model.orderSaving&&!model.orderDragging,expected).catch(async error=>{throw Error(name+': '+JSON.stringify(await order())+'; '+error.message)});check(true,name)};
 const begin=async(number,target,placement)=>{
   const handle=page.locator(`[data-move-issue="${number}"]`),row=page.locator(`.issue-row[data-issue-number="${target}"]`);await handle.scrollIntoViewIfNeeded();await row.scrollIntoViewIfNeeded();
   const h=await handle.boundingBox(),t=await row.boundingBox();await page.mouse.move(h.x+h.width/2,h.y+h.height/2);await page.mouse.down();await page.mouse.move(t.x+70,placement==='before'?t.y+8:t.y+t.height-8,{steps:12});
 };
 const drag=async(number,target,placement)=>{await begin(number,target,placement);await page.mouse.up();await page.waitForFunction(()=>!model.orderSaving&&!model.orderDragging)};
 await expect([1,2,3,4,5,6],'Initial order follows issue numbers');
 check((await page.locator('.list-sort').innerText()).includes('Queue order'),'Queue order label');
 await drag(6,1,'before');await expect([6,1,2,3,4,5],'Drag before first issue');
 await page.reload();await expect([6,1,2,3,4,5],'Order persists through reload');
 await drag(6,3,'after');await expect([1,2,3,6,4,5],'Drag after target');
 await page.locator('[data-move-issue="6"]').focus();await page.keyboard.press('ArrowUp');await expect([1,2,6,3,4,5],'Keyboard move up');
 check(await page.locator('[data-move-issue="6"]').evaluate(el=>document.activeElement===el),'Keyboard move preserves focus');
 await page.keyboard.press('ArrowDown');await expect([1,2,3,6,4,5],'Keyboard move down');
 await begin(5,1,'before');check(await page.locator('.issue-drop-before').count()===1,'Insertion indicator appears');await page.waitForTimeout(5100);
 check(await page.locator('.issue-drop-before').count()===1,'Polling does not interrupt a drag');await page.keyboard.press('Escape');await page.mouse.up();await expect([1,2,3,6,4,5],'Escape cancels drag');
 check(!await page.locator('body').evaluate(el=>el.classList.contains('issue-sorting')),'Cancel clears drag styles');
 await begin(5,1,'before');await page.evaluate(()=>document.querySelector('#issue-list').dispatchEvent(new PointerEvent('pointercancel',{pointerId:issueDrag.pointer,bubbles:true})));await page.mouse.up();await expect([1,2,3,6,4,5],'Pointer cancellation leaves order unchanged');
 let releaseRefresh,captureRefresh;const delayed=new Promise(resolve=>releaseRefresh=resolve),captured=new Promise(resolve=>captureRefresh=resolve);let held=false;
 await page.context().route('**/api/action',async route=>{const body=JSON.parse(route.request().postData());if(body.operation.action==='list'&&!held){held=true;captureRefresh();await delayed;const response=await route.fetch();await route.fulfill({response});}else await route.continue();});
 const revision=await page.evaluate(()=>model.orderVersion);await page.evaluate(()=>{window.orderDelayedRefresh=refresh(true)});await captured;
 await begin(5,2,'before');await page.evaluate(async()=>{await api({action:'move',number:4,before:1},model.project.id)});releaseRefresh();await page.evaluate(()=>window.orderDelayedRefresh);
 check(await page.evaluate(()=>model.orderVersion)===revision,'In-flight refresh preserves the rendered order revision during drag');
 check(JSON.stringify(await order())===JSON.stringify([1,2,3,6,4,5]),'In-flight refresh does not replace dragged rows');await page.context().unroute('**/api/action');
 await page.mouse.up();await expect([4,1,2,3,6,5],'Stale drag preserves concurrent reorder');
 check((await page.locator('#toast').innerText()).includes('Issue order changed'),'Concurrent conflict explains refresh');
 const retryIds=[];await page.context().route('**/api/action',async route=>{
   const body=JSON.parse(route.request().postData());if(body.operation.action==='move'&&body.operation.number===5&&body.operation.before===6){retryIds.push(body.request_id);if(retryIds.length===1){await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{code:'transport_error',message:'Temporary disconnection'}})});return;}}await route.continue();
 });
 await drag(5,6,'before');await expect([4,1,2,3,6,5],'Failed save preserves canonical order');
 await drag(5,6,'before');await expect([4,1,2,3,5,6],'Repeating failed drag recovers');
 check(retryIds.length===2&&retryIds[0]===retryIds[1],'Failed move retries with the same request ID');await page.context().unroute('**/api/action');
 await page.locator('#label-filter').selectOption('ready');await expect([1,3,5],'Filtered list uses saved order');await drag(5,1,'before');await expect([5,1,3],'Filtered drag persists relative move');
 await page.locator('#label-filter').selectOption('');await expect([4,5,1,2,3,6],'Filtered move preserves hidden issue order');
 check(await page.locator('.issue-row[data-issue-number="3"] .issue-pr-link').count()===1,'PR link remains on reordered row');
 const record=await page.evaluate(async()=>api({action:'view',number:3},model.project.id));check(record.issue.body==='# Requirements\nPreserve markdown and PRs.','Reordering preserves markdown');
 await page.screenshot({path:'output/playwright/issue-order-desktop.png'});
 await page.setViewportSize({width:390,height:844});await drag(5,1,'after');await expect([4,1,5,2,3,6],'Drag works in narrow viewport');
 check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Mobile list fits viewport');
 check(await page.locator('.issue-order-handle').first().evaluate(el=>getComputedStyle(el).touchAction==='none'),'Drag handle captures touch gestures');
 await page.screenshot({path:'output/playwright/issue-order-mobile.png'});
 await page.reload();await expect([4,1,5,2,3,6],'Mobile order survives reload');
 check(errors.length===0,'No browser errors');return {passed:checks.length,checks,project,order:await order()};
}
