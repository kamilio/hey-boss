// Run via playwright-cli run-code with serve_creator_model_fixture.mjs.
async page => {
  const checks = [], errors = [];
  page.on('pageerror',error=>errors.push(error.message));
  await page.route('**/api/inbox',route=>route.fulfill({json:{ok:true,tasks:[],unread:0}}));
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const base='http://127.0.0.1:59734/#project=named%3ACreator%20model%20QA';
  for (const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      await page.goto(base);
      await page.locator('.issue-row').nth(4).waitFor();
      const row = page.locator('[data-issue-number="1"]');
      check((await row.innerText()).includes('by Codex · gpt-6-astra'),`${theme}/${width}: captured model in list`);
      check(!(await row.innerText()).includes('aaaaaaaa'),`${theme}/${width}: creator hash replaced`);
      check((await page.locator('[data-issue-number="4"]').innerText()).includes('Codex · aaaaaaaa'),`${theme}/${width}: legacy fallback`);
      check((await page.locator('[data-issue-number="5"]').innerText()).includes('by Boss'),`${theme}/${width}: human attribution`);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme}/${width}: list fits`);
      check(await page.locator('[data-issue-number="3"] .issue-meta').evaluate(e=>e.scrollWidth<=e.clientWidth+1),`${theme}/${width}: unbroken model is not clipped`);
      await page.screenshot({path:`output/playwright/issue134/${theme}-${width}-list.png`,fullPage:true});
      await row.locator('.issue-title').focus();
      await page.keyboard.press('Enter');
      await page.getByRole('heading',{name:'Preserve the model that discovered this issue #1'}).waitFor();
      check((await page.locator('.detail-meta').innerText()).includes('Codex · gpt-6-astra'),`${theme}/${width}: detail attribution`);
      await page.getByText('Issue details',{exact:true}).click();
      const origin = page.getByRole('region',{name:'Origin',exact:true});
      check((await origin.innerText()).includes('Codex · gpt-6-astra'),`${theme}/${width}: origin attribution`);
      const href = await origin.getByRole('link',{name:'View creator conversation'}).getAttribute('href');
      check(href.includes('run=session%3Aaaaaaaaa-bbbb-cccc-dddd-000000000001'),`${theme}/${width}: exact conversation target`);
      await origin.getByText('Session details',{exact:true}).click();
      check((await origin.innerText()).includes('aaaaaaaa-bbbb-cccc-dddd-000000000001'),`${theme}/${width}: session remains inspectable`);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme}/${width}: detail fits`);
      await page.screenshot({path:`output/playwright/issue134/${theme}-${width}-detail.png`,fullPage:true});
      await page.goto(base+'&issue=3');
      await page.locator('.detail-meta').waitFor();
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${theme}/${width}: long model detail fits`);
      check(await page.locator('.detail-meta').evaluate(e=>e.scrollWidth<=e.clientWidth+1),`${theme}/${width}: unbroken detail model is not clipped`);
    }
  }
  check(errors.length===0,'No browser errors: '+errors.join('; '));
  return {checks:checks.length,errors};
}
