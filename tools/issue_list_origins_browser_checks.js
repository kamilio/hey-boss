// Run via playwright-cli run-code with serve_issue_list_origins_fixture.mjs.
async page => {
  const checks=[], errors=[];
  page.on('pageerror', error => errors.push(error.message));
  // Inbox is deliberately absent from this isolated issue-only fixture.
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread:0}}));
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const base='http://127.0.0.1:59676/#project=named%3AList%20origin%20QA';
  for (const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      await page.goto(base);
      await page.locator('.issue-row').nth(3).waitFor();
      check(await page.locator('.issue-row').count()===4,`${theme}/${width}: all open rows survive invalid origins`);
      check(await page.locator('[data-issue-number="2"] .comment-count').getAttribute('title')==='2 comments',`${theme}/${width}: correct comment count`);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme}/${width}: list fits`);
      await page.screenshot({path:`output/playwright/issue76/${theme}-${width}-list.png`,fullPage:true});
      await page.locator('[data-issue-number="1"] .issue-title').focus();
      await page.keyboard.press('Enter');
      await page.getByRole('heading',{name:'Legacy issue with no creation context #1'}).waitFor();
      await page.getByText('Creation context was not recorded.').waitFor();
      check(await page.getByText('Historical issue content remains available.',{exact:true}).count()===1,`${theme}/${width}: body preserved`);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme}/${width}: detail fits`);
      await page.screenshot({path:`output/playwright/issue76/${theme}-${width}-detail.png`,fullPage:true});
      await page.goto(base);
      await page.getByRole('tab',{name:/Closed/}).click();
      await page.locator('[data-issue-number="3"]').waitFor();
      check(await page.locator('.issue-row').count()===1,`${theme}/${width}: closed NULL-origin row`);
      await page.goto(base);
      await page.getByLabel('Filter by assignee').selectOption('unassigned');
      await page.locator('[data-issue-number="2"]').waitFor({state:'hidden'});
      check(await page.locator('.issue-row').count()===3,`${theme}/${width}: owner filter survives invalid origins`);
      await page.getByLabel('Search issues').fill('Malformed');
      await page.locator('.issue-row').nth(1).waitFor({state:'hidden'});
      check(await page.locator('[data-issue-number="4"]').count()===1,`${theme}/${width}: damaged origin remains searchable`);
    }
  }
  await page.goto(base+'&issue=2');
  await page.getByRole('region',{name:'Origin',exact:true}).waitFor();
  check(await page.getByRole('region',{name:'Origin',exact:true}).getByText('Boss',{exact:true}).count()===1,'Populated origin is preserved');
  check(errors.length===0,'No browser exceptions: '+errors.join('; '));
  return {checks:checks.length,errors};
}
