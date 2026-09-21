// Run via playwright-cli on an isolated issue server or paired-device fixture.
// Start on / for native tests or /issues for paired-device tests.
async page => {
  const base = await page.evaluate(() => location.origin);
  const issuesPath = await page.evaluate(() => location.pathname === '/issues' ? '/issues' : '/');
  const checks = [];
  const check = (value, name) => { if (!value) throw Error(name); checks.push(name); };
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  // The fixture deliberately has no native Inbox service.
  await page.route('**/api/inbox*', route => route.fulfill({json:{ok:true,tasks:[]}}));
  for (const route of [issuesPath, '/mm', '/artifacts', '/agents']) {
    await page.setViewportSize({width:390,height:844});
    await page.goto(base + route + '#project=' + encodeURIComponent('named:Project 01'));
    await page.waitForFunction(() => document.querySelector('#project-name')?.textContent === 'Project 01');
    check(await page.locator('#project-name').evaluate(el => el.scrollWidth <= el.clientWidth), route + ' short project name is readable on iPhone');
    for (const [width,height] of [[320,568],[375,667],[390,844],[430,932],[844,390],[1440,1000]]) {
      await page.setViewportSize({width,height});
      if (width <= 640) check(await page.locator('#project-name').evaluate(el => el.scrollWidth <= el.clientWidth),
        `${route} ${width}: short project name is readable`);
      for (const colorScheme of ['light','dark']) {
        await page.emulateMedia({colorScheme});
        await page.locator('#project-trigger').click();
        const menu = await page.locator('#project-menu').boundingBox();
        check(menu.x >= 0 && menu.x + menu.width <= width + 1 && menu.y >= 0 && menu.y + menu.height <= height,
          `${route} ${width} ${colorScheme}: whole menu fits viewport`);
        check(await page.locator('#project-menu').evaluate(el => el.scrollWidth <= el.clientWidth),
          `${route} ${width} ${colorScheme}: menu has no horizontal overflow`);
        check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
          `${route} ${width} ${colorScheme}: page has no horizontal overflow`);
        const controls = ['#project-search','#toggle-hidden-projects'];
        if (await page.locator('#add-project').count()) controls.push('#add-project');
        for (const selector of controls) {
          check(await page.locator(selector).evaluate(el => {
            const r=el.getBoundingClientRect();
            return el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2));
          }), `${route} ${width} ${colorScheme}: ${selector} is not clipped`);
        }
        const options = page.locator('#project-options');
        check(await options.evaluate(el => el.clientHeight > 40 && el.scrollHeight > el.clientHeight),
          `${route} ${width} ${colorScheme}: project list scrolls`);
        await options.evaluate(el => {el.scrollTop=el.scrollHeight;});
        await page.locator('.project-option').last().click();
        check(await page.locator('#project-menu').isHidden(), `${route} ${width}: last project can be selected`);
        await page.locator('#project-trigger').click();
        await page.locator('#project-search').fill('Project 01');
        await page.keyboard.press('ArrowDown');
        await page.keyboard.press('Enter');
        await page.waitForFunction(() => document.querySelector('#project-name').textContent === 'Project 01');
        await page.locator('#project-trigger').click();
        if (width <= 640) {
          await page.locator('#project-search').fill('A very long project name');
          check(await page.locator('.project-option strong').evaluate(el => getComputedStyle(el).whiteSpace === 'normal'),
            `${route} ${width}: long project names can wrap`);
        }
        await page.locator('#project-search').fill('no-such-project');
        check(await page.locator('.menu-empty').isVisible(), `${route} ${width}: empty search state is visible`);
        await page.keyboard.press('Escape');
        check(await page.locator('#project-trigger').evaluate(el=>el===document.activeElement), `${route} ${width}: Escape restores focus`);
      }
    }
  }
  check(errors.length === 0, 'No browser runtime errors: ' + errors.join('; '));
  return {passed:checks.length,checks};
}
