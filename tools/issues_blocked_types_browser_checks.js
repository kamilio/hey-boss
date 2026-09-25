// Run via playwright-cli run-code --filename on an isolated native/paired fixture.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  const base = await page.evaluate(() => location.origin);
  const path = await page.evaluate(() => location.pathname === '/issues' ? '/issues' : '/');
  const surface = path === '/issues' ? 'paired' : 'native';
  if (surface === 'native') for (const name of ['app.js','app.css','blockers.js'])
    await page.route(`${base}/${name}`, route => route.fulfill({path:`src/issues/web/${name}`}));
  if (surface === 'native') await page.route(`${base}/`, async route => { const response = await route.fetch(); let body = await response.text(); if (!body.includes('id="blocked-filter"')) body = body.replace('<div class="issue-panel glass">', '<div id="blocked-filter-control" class="blocked-filter-control" hidden><label for="blocked-filter">Blocked reason</label><select id="blocked-filter" aria-label="Filter by blocked reason"><option value="">All blocked issues</option><option value="hold">On hold</option><option value="dependencies">Waiting for dependencies</option></select></div><div class="issue-panel glass">'); await route.fulfill({response,body}); });
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread_count:0}}));
  await page.goto(base + path);
  await page.waitForFunction(() => model.csrf && model.signature);
  const project = await page.evaluate(async () => {
    const project = 'Blocked reason ' + Date.now();
    for (const title of ['Confirm production access','Review release scope','Publish the mobile update','Approve rollout timing'])
      await api({action:'create',title,body:'Blocked reason visual regression fixture.',labels:['release']},project);
    await api({action:'block',number:2,force:false,comment:'Scope needs a decision.'},project);
    await api({action:'set_blockers',number:3,blockers:[1],force:false},project);
    await api({action:'set_blockers',number:4,blockers:[1],force:false},project);
    await api({action:'block',number:4,force:false,comment:null},project);
    return 'named:' + project;
  });
  const url = base + path + '#project=' + encodeURIComponent(project) + '&state=blocked';
  await page.goto(url);
  await page.waitForFunction(project => model.project.id === project && model.signature, project);
  const rows = () => page.locator('.issue-row').count();
  const filter = page.getByLabel('Filter by blocked reason');
  const filtered = async (value, count) => {
    await filter.selectOption(value);
    await page.waitForFunction(({value,count}) => model.route.blocked === value && model.signature && document.querySelectorAll('.issue-row').length === count, {value,count});
  };
  check(await rows() === 3, 'All blocked issues visible');
  check(await page.locator('.issue-state.on-hold').count() === 2, 'Both deliberate holds have red state icons');
  check(await page.locator('[data-issue-number="3"] .issue-state').getAttribute('aria-label') === 'Waiting for dependencies', 'Dependency state has accessible text');
  await filtered('hold',2);
  check(await page.locator('#list-summary').innerText() === '2 issues', 'Filtered count matches visible issues');
  await page.reload();
  await page.waitForFunction(() => model.signature);
  check(await rows() === 2 && await filter.inputValue() === 'hold', 'Hold filter survives reload');
  await page.locator('[data-issue-number="4"] .issue-title').click();
  await page.locator('.state-pill.on-hold').waitFor();
  check(await page.locator('.blocked-notice h2').innerText() === 'On hold', 'Detail uses the same name');
  await page.getByRole('button',{name:'All issues',exact:true}).click();
  await page.waitForFunction(() => model.signature);
  check(await filter.inputValue() === 'hold' && await rows() === 2, 'Back to list preserves filter');
  await filtered('dependencies',2);
  check(await page.locator('[data-issue-number="4"]').count() === 1, 'Mixed blocker appears under dependencies too');
  await filtered('',3);
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${scheme}/${width}: list fits`);
      const colors = await page.evaluate(() => [2,3].map(n => getComputedStyle(document.querySelector(`[data-issue-number="${n}"] .issue-state`)).color));
      check(colors[0] !== colors[1], `${scheme}/${width}: hold and dependency colors differ`);
      check(await filter.evaluate(el => {const r=el.getBoundingClientRect();return r.width>180 && r.right<=innerWidth && r.height>=40;}), `${scheme}/${width}: readable filter target`);
      await page.screenshot({path:`output/playwright/issue254/${surface}-${scheme}-${width}-list.png`,fullPage:true});
      await page.locator('[data-issue-number="4"] .issue-title').click();
      await page.locator('.blocked-notice.on-hold').waitFor();
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${scheme}/${width}: mixed blocker detail fits`);
      await page.screenshot({path:`output/playwright/issue254/${surface}-${scheme}-${width}-detail.png`,fullPage:true});
      await page.getByRole('button',{name:'All issues',exact:true}).click();
      await page.waitForFunction(() => model.signature);
    }
  }
  await filtered('hold',2);
  await page.locator('#issue-search').fill('No matching title');
  await page.getByRole('heading',{name:'No matching issues'}).waitFor();
  await page.getByRole('button',{name:'Clear filters',exact:true}).click();
  await page.waitForFunction(() => model.signature && document.querySelectorAll('.issue-row').length === 3);
  check(await filter.inputValue() === '', 'Empty state clears the blocked reason too');
  await filtered('hold',2);
  await page.locator('[data-issue-number="4"] .issue-title').click();
  await page.locator('.blocked-notice [data-action="clear_manual_hold"]').click();
  await page.waitForFunction(() => model.detail?.issue.state === 'blocked' && !model.detail.issue.manual_blocked);
  check(await page.locator('.state-pill').innerText() === 'Waiting for dependencies', 'Release hold keeps active dependencies blocked');
  await page.getByRole('button',{name:'All issues',exact:true}).click();
  await page.waitForFunction(() => model.signature && document.querySelectorAll('.issue-row').length === 1);
  check(await rows() === 1, 'Released hold disappears from hold filter');
  await page.locator('[data-state="open"]').click();
  await page.waitForFunction(() => model.route.state === 'open' && model.signature);
  check(await filter.isHidden() && await rows() === 1, 'Open tab resets and hides blocked filtering');
  check(errors.length === 0, `No browser exceptions: ${errors.join('; ')}`);
  return {surface,passed:checks.length,errors};
}
