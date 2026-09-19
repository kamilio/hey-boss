// Run with Playwright CLI against the isolated tools/inbox_fixture.swift service.
async (page) => {
  const checks = [];
  const check = (ok, label) => { if (!ok) throw Error(label); checks.push(label); };
  await page.goto(page.url().split('/').slice(0,3).join('/') + '/#view=inbox');
  await page.waitForFunction(() => inboxTasks.length === 6);
  check(await page.evaluate(() => inboxTasks.find(t => t.taskID === 'notice-5').icon === 'deploy'),
    'Native list preserves the selected icon');
  await page.evaluate(() => {
    inboxBusy = true; // Keep background refresh from replacing synthetic appearance cases.
    const base = {kind: 'alert', status: 'pending', project: 'Inbox QA', sourceHost: 'This Mac', createdAt: Date.now()/1000,
      summary: 'The report is ready. Open this notification for details.'};
    inboxTasks = ['neutral', 'info', 'success', 'warning', 'error'].map((severity, i) => ({...base,
      severity, taskID: 'style-' + i, title: severity.charAt(0).toUpperCase() + severity.slice(1) + ' notification'}));
    inboxTasks.push({...base, taskID: 'style-custom', severity: 'warning', icon: 'build', title: 'Build needs attention'});
    inboxTasks.push({...base, taskID: 'style-image', severity: 'success', iconData:
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aSs0AAAAASUVORK5CYII=', title: 'Custom image notification'});
    renderInboxList();
  });
  check(await page.evaluate(() => {
    const cases = [ ['update', 'docs'], ['alert', 'bell'], ['prompt', 'question'], ['approval', 'question'] ];
    return cases.every(([kind, expected]) => noticeIcon({kind}) === expected)
      && noticeSeverity({severity: 'invalid'}) === 'neutral'
      && noticeIcon({kind:'alert', severity:'error', icon:'not.a.symbol'}) === 'error'
      && noticeIcon({severity:'warning', icon:'hammer.fill'}) === 'build'
      && !noticeBadge({iconData:'https://example.com/icon.png'}).includes('<img')
      && !noticeBadge({iconData:'iVBORw0KGgo\" onerror=alert(1)'}).includes('<img');
  }), 'Kind defaults, invalid values, SF Symbol aliases, and unsafe image values fall back safely');
  check(await page.locator('.notice-symbol[aria-label="Warning"]').count() === 2,
    'Severity is exposed without relying on color');
  check(await page.locator('[data-notice-row="style-custom"] .notice-status-mark').count() === 1,
    'Selected symbol retains a warning badge');
  check(await page.locator('[data-notice-row="style-image"] .notice-symbol img').count() === 1,
    'Saved PNG and severity badge render together');
  for (const scheme of ['light', 'dark']) {
    await page.emulateMedia({colorScheme: scheme});
    await page.setViewportSize({width: 1280, height: 1080});
    check(await page.evaluate(() => {
      const rows = [...document.querySelectorAll('.notice-row')];
      return new Set(rows.slice(0,5).map(r => getComputedStyle(r).borderLeftColor)).size === 5
        && rows.slice(1,5).every(r => getComputedStyle(r).backgroundImage.includes('linear-gradient'));
    }), scheme + ' distinguishes every severity with a stripe and surface wash');
    await page.screenshot({path: 'output/playwright/inbox-native-' + scheme + '.png', fullPage: true});
    await page.setViewportSize({width: 375, height: 950});
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), scheme + ' mobile list fits viewport');
    await page.screenshot({path: 'output/playwright/inbox-native-mobile-' + scheme + '.png', fullPage: true});
  }
  await page.evaluate(() => {
    const task = {...inboxTasks[5], status: 'ok', body_html: '<p>Build report.</p>'};
    renderNotice(task);
  });
  check(await page.locator('.notice-card.warning .notice-status-mark').count() === 1,
    'Archived detail preserves severity and selected icon');
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Mobile detail fits viewport');
  console.log(JSON.stringify({checks: checks.length, passed: checks}));
}
