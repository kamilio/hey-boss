async page => {
  const checks=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
  const id=await page.evaluate(()=>inboxDetail.taskID);
  const task=()=>page.evaluate(id=>inboxApi({action:'view',task_id:id}).then(v=>v.task),id);
  const overflow=()=>page.evaluate(()=>document.documentElement.scrollWidth>innerWidth);
  for (const [width,height,theme] of [[1440,1050,'light'],[1440,1050,'dark'],[390,844,'light'],[390,844,'dark'],[320,800,'light']]) {
    await page.setViewportSize({width,height});await page.emulateMedia({colorScheme:theme});
    check(!await overflow(),`${width} ${theme} has no page overflow`);
    check(await page.getByRole('button',{name:'Approve once',exact:true}).isVisible(),`${width} ${theme} approval remains visible`);
    check(await page.getByRole('button',{name:'Decline',exact:true}).isVisible(),`${width} ${theme} decline remains visible`);
    check(await page.getByRole('button',{name:'Cancel',exact:true}).isVisible(),`${width} ${theme} cancel remains visible`);
    await page.screenshot({path:`/Users/kjopek/Workspace/hey-boss/output/playwright/issue61/files-${width}-${theme}.png`,fullPage:true});
  }
  check((await task()).status==='pending','Viewing and resizing never answer the request');
  check(await page.locator('.related-issue-link').count()===1,'Approval links to its issue');
  const text=await page.locator('#inbox-detail').textContent().catch(()=>page.locator('main').textContent());
  check(text.includes('/synthetic/repo/example.rs')&&text.includes('-old')&&text.includes('+new'),'File diff and path are readable');
  await page.setViewportSize({width:1440,height:1050});
  await page.getByRole('button',{name:'Approve once',exact:true}).focus();
  check(await page.getByRole('button',{name:'Approve once',exact:true}).evaluate(el=>el===document.activeElement),'Decision supports keyboard focus');
  await page.reload();await page.waitForSelector('[data-notice-answer="Approve once"]');
  check((await task()).status==='pending','Reload preserves the pending decision');
  return checks;
}
