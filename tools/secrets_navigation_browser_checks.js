// Run with playwright-cli against an isolated issue web server. All inventory data is synthetic.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  const origin = await page.evaluate(() => location.origin);
  const projects = [{id:'named:Alpha',name:'Alpha'}, {id:'named:Beta',name:'Beta'}];
  await page.route('**/api/bootstrap', route => route.fulfill({json:{ok:true,projects,project:projects[0],csrf:'synthetic-test',actor:{id:'human:boss'}}}));
  let reads = 0;
  await page.route('**/api/secrets', route => {
    reads++;
    check(!route.request().url().includes('?'), 'Inventory request has no project scope');
    return route.fulfill({json:{records:[],machines:[],backend:'keychain',machine:null}});
  });
  for (const width of [1440,390,320]) for (const colorScheme of ['light','dark']) {
    await page.setViewportSize({width,height:900});
    await page.emulateMedia({colorScheme});
    await page.goto(origin+'/secrets');
    await page.waitForFunction(() => document.querySelector('#status').textContent.startsWith('Local snapshot'));
    await page.waitForFunction(() => document.querySelector('#project-name').textContent !== 'Projects');
    check(await page.locator('.app-navigation #nav-secrets').count() === 0, 'Secrets is outside project tabs');
    check(await page.locator('#navigation-more-links #nav-secrets').count() === 1, 'Secrets uses the shared global dropdown');
    check(await page.locator('#nav-secrets').getAttribute('href') === '/secrets', 'Global link has no project context');
    check(await page.locator('#nav-secrets').getAttribute('aria-current') === 'page', 'Global link marks the current page');
    check(await page.locator('#navigation-more svg').count() === 1, 'Shared menu icon is rendered');
    await page.locator('#navigation-more').click();
    check(await page.locator('#nav-secrets').isVisible(), 'Dropdown opens on Secrets');
    const bounds = await page.locator('#navigation-more-links').boundingBox();
    check(bounds.x >= 0 && bounds.x + bounds.width <= width, 'Global dropdown fits the viewport');
    await page.keyboard.press('Escape');
    const before = reads;
    await page.locator('#project-trigger').click();
    await page.locator('#project-search').fill('Beta');
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');
    check(await page.locator('#project-name').textContent() === 'Beta', 'Shared picker works with keyboard');
    check(await page.locator('#nav-issues').getAttribute('href') === '/#project=named%3ABeta', 'Project navigation remembers selection');
    check(await page.locator('#nav-secrets').getAttribute('href') === '/secrets' && reads === before, 'Switching projects leaves global inventory unchanged');
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'No horizontal overflow');
    await page.locator('#project-trigger').click();
    await page.keyboard.press('Escape');
    check(await page.locator('#project-trigger').evaluate(el => el === document.activeElement), 'Picker restores keyboard focus');
  }
  check(errors.length === 0, 'No browser runtime errors: '+errors.join('; '));
  return {passed:checks.length,checks};
}
