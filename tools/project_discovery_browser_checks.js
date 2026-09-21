// Run with playwright-cli against the isolated project picker fixture, native or paired.
async page => {
  const base = await page.evaluate(() => location.origin);
  const paired = await page.evaluate(() => location.pathname === '/issues');
  const bootstrap = await (await page.request.get(base + '/api/bootstrap')).json();
  const action = async (project, operation) => {
    const response = await page.request.post(base + '/api/action', {
      headers: {'X-Hey-Boss-CSRF': bootstrap.csrf},
      data: {project, operation, request_id: null},
    });
    if (!response.ok()) throw Error(await response.text());
    const result = await response.json();
    if (!result.ok) throw Error(JSON.stringify(result));
    return result;
  };
  const repository = 'github.com/example/hey-boss';
  const temporary = 'local:remote:/tmp/hey-boss-empty-browser-fixture';
  const saved = 'local:remote:/tmp/saved-browser-fixture';
  await action(repository, {action:'create',title:'Shared repository queue',body:'Worktrees use this same queue.',labels:[]});
  await action(temporary, {action:'list',state:'open',mine:false,unassigned:false,labels:[],search:null,limit:50,offset:0});
  await action(saved, {action:'create',title:'Retained temporary project',body:'Saved work stays accessible.',labels:[]});
  const checks = [], errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const check = (value, label) => { if (!value) throw Error(label); checks.push(label); };
  const registry = await action(repository, {action:'projects',include_hidden:true});
  check(!registry.projects.some(p => p.id === temporary), 'Legacy empty temporary project omitted');
  check(registry.projects.some(p => p.id === saved), 'Temporary project with saved work retained');
  check(registry.projects.filter(p => p.id === repository).length === 1, 'One canonical repository entry');
  await page.route('**/api/inbox*', route => route.fulfill({json:{ok:true,tasks:[]}}));
  for (const route of [paired ? '/issues' : '/', '/mm', '/artifacts', '/agents']) {
    for (const [width,height] of [[320,568],[390,844],[844,390],[1440,1000]]) {
      await page.setViewportSize({width,height});
      for (const colorScheme of ['light','dark']) {
        await page.emulateMedia({colorScheme});
        await page.goto(base + route + '#project=' + encodeURIComponent(repository));
        await page.waitForFunction(() => document.querySelector('#project-name')?.textContent === 'hey-boss');
        await page.locator('#project-trigger').click();
        await page.locator('#project-search').fill('hey-boss');
        const options = page.locator('.project-option');
        check(await options.count() === 1, `${route} ${width} ${colorScheme}: one hey-boss entry`);
        check(await options.getAttribute('data-project') === repository, `${route} ${width} ${colorScheme}: correct repository`);
        const menu = await page.locator('#project-menu').boundingBox();
        check(menu.x >= 0 && menu.y >= 0 && menu.x + menu.width <= width + 1 && menu.y + menu.height <= height + 1,
          `${route} ${width} ${colorScheme}: menu fits viewport`);
        check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${route} ${width} ${colorScheme}: no horizontal overflow`);
        if ((width === 390 || width === 1440) && (route === '/' || route === '/issues'))
          await page.screenshot({path:`output/playwright/issue84/${paired ? 'paired' : 'native'}-${width}-${colorScheme}.png`});
        await page.keyboard.press('ArrowDown');
        await page.keyboard.press('Enter');
        check(await page.locator('#project-menu').isHidden(), `${route} ${width} ${colorScheme}: keyboard selection works`);
        await page.locator('#project-trigger').click();
        await page.locator('#project-search').fill('saved-browser-fixture');
        check(await page.locator('.project-option').count() === 1, `${route} ${width} ${colorScheme}: saved work can be found`);
        await page.keyboard.press('Escape');
        check(await page.locator('#project-trigger').evaluate(el => el === document.activeElement), `${route} ${width} ${colorScheme}: focus restored`);
      }
    }
  }
  check(errors.length === 0, 'No browser errors: ' + errors.join('; '));
  return {passed:checks.length,checks};
}
