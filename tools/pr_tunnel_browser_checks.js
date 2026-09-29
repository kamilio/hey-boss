// Run with playwright-cli after chief_metadata_checks.mjs --pr-attachments --serve.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  const detail = 'http://127.0.0.1:59651/#project=named%3AChief%20metadata%20QA&issue=2';
  try {
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme});
      for (const width of [1440, 768, 390, 320]) {
        await page.setViewportSize({width, height:900});
        await page.goto(detail, {waitUntil:'commit'});
        await page.waitForFunction(() => typeof model !== 'undefined' && model.detail?.issue.number === 2);
        for (const [number, purpose] of [[123,'prerequisite'],[124,'supporting-evidence'],[125,'fix']]) {
          const url = `https://github.com/example/supervisor-tunnel/pull/${number}`;
          const select = page.getByRole('combobox', {name:`Purpose of PR ${url}`, exact:true});
          check(await select.inputValue() === purpose, `Purpose ${number} ${colorScheme} ${width}`);
          const link = page.locator('.pr-link-heading a').filter({hasText:url});
          check(await link.getAttribute('href') === url, `Destination ${number} ${colorScheme} ${width}`);
          await select.focus();
          check(await select.evaluate(el => el === document.activeElement), `Keyboard focus ${number} ${colorScheme} ${width}`);
        }
        check(await page.evaluate(() => model.detail.issue.assignee === 'codex:active-worker'), `Assignment ${colorScheme} ${width}`);
        check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth && [...document.querySelectorAll('.pr-link, .pr-purpose-field')].every(el => el.scrollWidth <= el.clientWidth + 1 && el.getBoundingClientRect().right <= innerWidth)), `No link overflow ${colorScheme} ${width}`);
        if ([1440,320].includes(width)) await page.screenshot({path:`output/playwright/pr-tunnel/${colorScheme}-${width}.png`, fullPage:true});
        await page.getByRole('button', {name:'All issues', exact:true}).click();
        await page.waitForFunction(() => !model.route.issue);
        const row = page.locator('[data-issue-number="2"]');
        check(await row.locator('.issue-pr-link').count() === 3, `Three list links ${colorScheme} ${width}`);
        check((await row.innerText()).includes('Prerequisite') && (await row.innerText()).includes('Supporting evidence'), `List purpose labels ${colorScheme} ${width}`);
      }
    }
    await page.goto(detail, {waitUntil:'commit'});
    await page.reload({waitUntil:'commit'});
    await page.waitForFunction(() => model.detail?.issue.pull_requests.length === 3);
    check(errors.length === 0, 'No browser runtime errors');
    return {completed:checks.length, checks};
  } finally { page.off('pageerror', onError); }
}
