// Run with playwright-cli run-code --filename after chief_metadata_checks.mjs --reopen --serve.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  try {
    await page.bringToFront();
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme});
      for (const width of [1440, 768, 390, 320]) {
        await page.setViewportSize({width, height:900});
        for (const [number, state] of [[5,'blocked'], [7,'blocked'], [8,'open'], [2,'open']]) {
          await page.goto(`http://127.0.0.1:59651/#project=named%3AChief%20metadata%20QA&issue=${number}`, {waitUntil:'commit'});
          await page.waitForFunction(number => typeof model !== 'undefined' && model.detail?.issue.number === number, number, {polling:100});
          check(await page.evaluate(state => model.detail.issue.state === state, state), `State ${colorScheme} ${width} #${number}`);
          check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth && [...document.querySelectorAll('.detail-top, .detail-layout, .issue-blockers')].every(el => el.getBoundingClientRect().right <= innerWidth && el.scrollWidth <= el.clientWidth + 1)), `Fits ${colorScheme} ${width} #${number}`);
          check(await page.locator('[data-edit]').isVisible(), `Edit reachable ${colorScheme} ${width} #${number}`);
          if ([5,7].includes(number)) {
            check(await page.locator('.issue-blockers').innerText().then(text => text.includes('Unfinished prerequisite') && text.includes('Unfinished issue')), `Dependency visible ${colorScheme} ${width} #${number}`);
            check(await page.locator('[data-action="reopen"]').isDisabled(), `Dependency prevents pickup ${colorScheme} ${width} #${number}`);
          }
          check(await page.evaluate(number => model.detail.issue.assignee === (number === 2 ? 'codex:active-worker' : null), number), `Ownership ${colorScheme} ${width} #${number}`);
          if ([1440,320].includes(width) && [5,8,2].includes(number)) await page.screenshot({path:`output/playwright/issue369/${colorScheme}-${width}-${number}.png`, fullPage:true});
        }
      }
    }
    check(errors.length === 0, 'No runtime errors');
    return {completed:checks.length,checks};
  } finally { page.off('pageerror', onError); }
}
