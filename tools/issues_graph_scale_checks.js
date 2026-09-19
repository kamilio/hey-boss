// Run on the isolated 5,000-issue / 4,950-edge graph fixture.
async page => {
 const origin=await page.evaluate(()=>location.origin),checks=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
 await page.setViewportSize({width:1440,height:1000});await page.emulateMedia({colorScheme:'light'});
 const start=Date.now();await page.goto(origin+'/#project=named%3AScale%20test');await page.waitForFunction(()=>model.project?.id==='named:Scale test'&&document.querySelectorAll('#issue-list .issue-row').length===5000);const listLoad=Date.now()-start;
 check(await page.locator('#issue-list .issue-parent-link').count()===4950,'Every subtask keeps its parent link in the full queue');
 check(await page.locator('#issue-list .issue-subtask-progress').count()===50,'All 50 parents expose direct progress');
 check(await page.locator('#issue-list [data-issue-number="1"] .issue-subtask-progress').textContent()==='0/99','Large graph progress is accurate');
 check(await page.locator('#previous-page,#next-page').count()===0,'Large graph still has no pagination');
 await page.locator('#issue-search').fill('fixture 4999');await page.waitForFunction(()=>model.issues?.length===1);
 check(await page.locator('#issue-list .issue-parent-link').innerText()==='#50','Filtered child preserves its parent context');
 const detailStart=Date.now();await page.locator('#issue-list .issue-title').click();await page.waitForFunction(()=>model.detail?.issue.number===4999);const detailLoad=Date.now()-detailStart;
 check(await page.locator('.parent-breadcrumb').innerText().then(t=>t.includes('#50 Performance fixture 50')),'Deep queue issue opens its correct parent');
 await page.locator('.parent-breadcrumb a').click();await page.waitForFunction(()=>model.detail?.issue.number===50&&model.detail?.subtasks?.length===99);
 check(await page.locator('#subtask-list > li').count()===99,'Parent exposes every direct child');
 const nums=await page.locator('#subtask-list > li').evaluateAll(rows=>rows.map(r=>Number(r.dataset.issueNumber)));check(nums.every((n,i)=>i===0||n>nums[i-1]),'Sibling order follows the CLI queue');
 for(const width of [320,390,768]){await page.setViewportSize({width,height:844});check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Large graph fits '+width+'px');}
 await page.locator('[data-add-existing-subtask]').click();await page.waitForFunction(()=>document.querySelectorAll('[data-existing-subtask]').length===5000);check(await page.locator('[data-existing-subtask]').count()===5000,'Existing issue picker includes the entire queue');
 await page.locator('#subtask-picker-search').fill('fixture 4999');check(await page.locator('[data-existing-subtask]').count()===1,'Large picker filters immediately');
 check(await page.locator('[data-existing-subtask="4999"]').isDisabled(),'Large picker respects existing ownership by parent');await page.keyboard.press('Escape');await page.setViewportSize({width:1440,height:1000});
 return {passed:checks.length,checks,list_load_ms:listLoad,detail_load_ms:detailLoad};
}
