// Run with Playwright CLI on an isolated issue web server containing two projects.
async (page) => {
  const origin = await page.evaluate(() => location.origin);
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  const now = Date.now() / 1000;
  await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread:0}}));
  await page.route('**/api/fleet/status', route => route.fulfill({json:{ok:true,
    machines:[{host:'review-machine.local',hostname:'Review MacBook',role:'controller',state:'connected',heartbeat:now,
      workers:[{id:'review-worker',active:0,eligible:2,config:{name:'Design review worker',enabled:true,concurrency:2},runs:[]}]}],events:[],conflicts:[],signals:[]}}));
  for (const colorScheme of ['light', 'dark']) {
    await page.emulateMedia({colorScheme});
    for (const width of [320, 390, 768, 1440]) {
      await page.setViewportSize({width,height:900});
      let reference;
      for (const path of ['/', '/#view=inbox', '/workers', '/mm']) {
        await page.goto(origin + path, {waitUntil:'domcontentloaded'});
        await page.waitForFunction(() => document.querySelector('#project-name')?.textContent !== 'Projects');
        if (path.includes('inbox')) await page.waitForSelector('#inbox-search');
        if (path === '/workers') await page.waitForSelector('.machine');
        if (path === '/mm') await page.waitForSelector('#caption');
        const style = await page.evaluate(() => {
          const heading = [...document.querySelectorAll('.page-heading')].find(e => e.getBoundingClientRect().height);
          const main = document.querySelector('main'), h1 = heading.querySelector('h1'), panel = document.querySelector('.glass');
          const rect = main.getBoundingClientRect(), title = h1.getBoundingClientRect();
          return {mainX:rect.x,mainWidth:rect.width,titleX:title.x,titleY:title.y,font:getComputedStyle(h1).fontSize,
            background:getComputedStyle(document.body).background, text:getComputedStyle(h1).color,
            radius:getComputedStyle(panel).borderRadius, overflow:document.documentElement.scrollWidth-innerWidth,
            current:document.querySelectorAll('.app-navigation a[aria-current="page"]').length};
        });
        reference ||= style;
        for (const property of ['mainX','mainWidth','titleX','titleY','font','background','text','radius']) {
          check(style[property] === reference[property], `${colorScheme} ${width} ${path}: shared ${property}`);
        }
        check(style.overflow <= 1, `${colorScheme} ${width} ${path}: no horizontal overflow (${style.overflow})`);
        check(style.current === 1, `${colorScheme} ${width} ${path}: one current navigation item`);
        if (path.includes('inbox')) continue; // Inbox spans projects and hides the picker.
        await page.locator('#project-trigger').click();
        check(await page.locator('#project-search').evaluate(e => e === document.activeElement), `${path}: picker focuses search`);
        await page.keyboard.press('Escape');
        check(await page.locator('#project-menu').isHidden(), `${path}: Escape closes picker`);
      }
    }
  }
  await page.goto(origin + '/workers');
  await page.waitForSelector('.machine');
  await page.locator('#project-trigger').click();
  await page.locator('#project-search').fill('Borealis');
  await page.locator('.project-option').click();
  await page.waitForFunction(() => document.querySelector('#project-name').textContent === 'Borealis');
  check((await page.locator('#nav-issues').getAttribute('href')).includes('named%3ABorealis'), 'Workers picker preserves project in Issues navigation');
  check((await page.locator('#nav-mindmaps').getAttribute('href')).includes('named%3ABorealis'), 'Workers picker preserves project in Mindmaps navigation');
  check(await page.locator('.machine').count() === 1, 'Project selection keeps whole fleet visible');
  check(errors.length === 0, `No browser exceptions: ${errors.join('; ')}`);
  await page.screenshot({path:'output/playwright/issue18-workers-desktop-dark.png',fullPage:true});
  await page.setViewportSize({width:390,height:844});
  await page.screenshot({path:'output/playwright/issue18-workers-phone-dark.png',fullPage:true});
  return {passed:checks.length,errors};
}
