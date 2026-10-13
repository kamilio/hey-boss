// Run on the paired root of tools/serve_list_pagination_fixture.mjs.
async page => {
  const checks=[],errors=[];
  const check=(value,label)=>{if(!value)throw Error(label);checks.push(label);};
  await page.unrouteAll({behavior:'ignoreErrors'});
  page.on('pageerror',error=>errors.push(error.message));
  const base=await page.evaluate(()=>location.origin);
  let tasks=Array.from({length:4010},(_,i)=>({taskID:'mobile-'+i,title:`Phone notification ${String(i%2005+1).padStart(4,'0')}`,project:'Pagination QA',kind:'alert',severity:'success',status:i<2005?'pending':'resolved',sourceHost:'MacBook',description:'Release verification completed.',question:'All checks passed.',options:[],createdAt:Date.now()/1000-i,completedAt:i<2005?null:Date.now()/1000-i,handledBy:'mac'}));
  await page.route('**/api/tasks',route=>route.fulfill({json:{tasks,connected:true,pushEnabled:true,notifications:{mode:'automatic',macState:'active'}}}));
  await page.route('**/api/tasks/mobile-*',route=>route.fulfill({json:{task:tasks.find(task=>route.request().url().includes(task.taskID))}}));
  await page.route('**/api/tasks/mobile-*/open',route=>route.fulfill({json:{task:{taskID:'mobile-1000',status:'resolved'}}}));
  await page.goto(base+'/');
  const top=()=>page.locator('.list-pagination').first();
  await page.waitForFunction(()=>document.querySelectorAll('.task-card').length===1000);
  check((await top().innerText()).includes('1–1,000'),'Phone Inbox is bounded to 1,000');
  await top().getByRole('button',{name:'Next page of notices'}).click();
  await page.waitForFunction(()=>document.querySelector('.task-card h2')?.textContent==='Phone notification 1001');
  check(await page.evaluate(()=>document.activeElement.classList.contains('list-pagination')),'Phone page change restores focus');
  await page.locator('.task-open').first().click();
  await page.getByRole('button',{name:'Close reader',exact:true}).click();
  check(await page.locator('.task-card h2').first().innerText()==='Phone notification 1001','Reader preserves phone page');
  await top().getByRole('button',{name:'Next page of notices'}).click();
  await page.waitForFunction(()=>document.querySelectorAll('.task-card').length===5);
  check(await top().getByRole('button',{name:'Next page of notices'}).isDisabled(),'Final phone page disables Next');
  await page.getByRole('button',{name:'History',exact:true}).click();
  await page.waitForFunction(()=>document.querySelectorAll('.activity-row').length===1000);
  check(true,'Phone history is bounded to 1,000');
  await top().getByRole('button',{name:'Next page of notices'}).click();
  await page.waitForFunction(()=>document.querySelector('.activity-row strong')?.textContent==='Phone notification 1001');
  await top().getByRole('button',{name:'Next page of notices'}).click();
  await page.waitForFunction(()=>document.querySelectorAll('.activity-row').length===5);
  check(await page.locator('.activity-row strong').last().innerText()==='Phone notification 2005','Phone history final page has all remaining notices');
  tasks=tasks.filter(task=>task.status==='pending'||Number(task.taskID.slice(7))<3005);
  await page.getByRole('button',{name:'Refresh inbox',exact:true}).click();
  await page.waitForFunction(()=>document.querySelectorAll('.activity-row').length===1000 && !document.querySelector('.list-pagination'));
  check(true,'Phone history clamps after shrinking');
  await page.getByRole('button',{name:'Unread',exact:false}).click();
  for(const theme of ['light','dark'])for(const width of [390,320]){
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    await page.setViewportSize({width,height:844});
    await page.evaluate(()=>scrollTo(0,0));
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Phone fits ${theme}/${width}`);
    check(await top().evaluate(el=>getComputedStyle(el).position!=='fixed'),`Pager stays in flow ${theme}/${width}`);
    await page.screenshot({path:`/tmp/hb-list-pagination-phone-${theme}-${width}.png`});
  }
  await page.getByRole('button',{name:'Clear all',exact:true}).click();
  check((await page.getByRole('alertdialog').count() || await page.getByRole('dialog').count())>0,'Clear-all confirmation remains available');
  check((await page.locator('.clear-inbox-dialog').innerText()).includes('2005 notices'),'Clear all covers the full Inbox, not just the visible page');
  await page.getByRole('button',{name:'Keep unread'}).click();
  check(errors.length===0,'No mobile runtime errors: '+errors.join('; '));
  return {checks:checks.length,passed:checks};
}
