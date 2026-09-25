// Run with playwright-cli run-code --filename after chief_metadata_checks.mjs --serve.
async page => {
  const checks = [], errors = [];
  const check = (value, name) => { if (!value) throw Error(name); checks.push(name); };
  const onError = error => errors.push(error.message);
  const open = async number => {
    await page.bringToFront();
    await page.goto(`http://127.0.0.1:59651/#project=named%3AChief%20metadata%20QA&issue=${number}`, {waitUntil:'commit'});
    await page.waitForFunction(number => typeof model !== 'undefined' && model.detail?.issue.number === number, number, {polling:100});
  };
  page.on('pageerror', onError);
  try {
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme});
      for (const width of [1440, 768, 390, 320]) {
        await page.setViewportSize({width, height:900});
        for (const number of [2, 3]) {
          await open(number);
          check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth && Array.from(document.querySelectorAll('.detail-top, .detail-layout, .draft-notice, .compose-actions')).every(el => el.getBoundingClientRect().right <= innerWidth && el.scrollWidth <= el.clientWidth + 1)), `No overflow ${colorScheme} ${width} #${number}`);
          check(await page.locator('[data-edit]').isVisible(), `Edit reachable ${colorScheme} ${width} #${number}`);
          check(await page.evaluate(number => model.detail.issue.draft === (number === 3) && model.detail.issue.assignee === (number === 3 ? null : 'codex:active-worker'), number), `Draft and assignment retained ${colorScheme} ${width} #${number}`);
          if (number === 3) {
            check(await page.getByRole('heading', {name:'This issue is a draft'}).isVisible(), `Draft readiness notice ${colorScheme} ${width}`);
            check(await page.locator('[data-draft-action="ready"]').isVisible(), `Ready action fits ${colorScheme} ${width}`);
          }
          if ([1440,320].includes(width)) await page.screenshot({path:`output/playwright/issue152/${colorScheme}-${width}-${number}.png`, fullPage:width > 320});
        }
      }
      await open(2);
      await page.locator('[data-edit]').click();
      await page.locator('#editor-subject').fill('Keep this unsaved edit');
      await page.locator('#editor-submit').click();
      await page.locator('#editor-error').waitFor({state:'visible'});
      const error = await page.locator('#editor-error').innerText();
      check(error.includes('hey-boss fleet capabilities') && error.includes('--supervisor'), `Actionable tunnel recovery ${colorScheme}`);
      check(await page.locator('#editor-subject').inputValue() === 'Keep this unsaved edit', `Rejected edit retained ${colorScheme}`);
      check(await page.locator('#editor-error').evaluate(el => el.scrollWidth <= el.clientWidth), `Recovery fits phone ${colorScheme}`);
      await page.screenshot({path:`output/playwright/issue152/${colorScheme}-recovery.png`});
      await page.locator('#editor-cancel').click();
      check(await page.locator('[data-edit]').evaluate(el => el === document.activeElement), `Cancel restores keyboard focus ${colorScheme}`);
    }
    await open(4);
    await page.waitForFunction(() => model.detail?.issue.number === 4 && !model.detail.issue.draft, null, {polling:100});
    await page.locator('[data-draft-action="draft"]').click();
    await page.getByRole('heading', {name:'This issue is a draft'}).waitFor();
    check(await page.evaluate(() => model.detail.issue.draft && model.detail.issue.assignee === null), 'Browser draft uses tunnel without a claim');
    // Ordinary reads are replica snapshots. Verify persistence after the next
    // pull; a hard reload intentionally has no in-memory mutation response.
    await page.waitForFunction(async () => (await api({action:'view',number:4})).issue.draft, null, {polling:250});
    await page.reload({waitUntil:'commit'});
    await page.getByRole('heading', {name:'This issue is a draft'}).waitFor();
    check(await page.evaluate(() => model.detail.issue.draft), 'Browser draft survives reload');
    await page.screenshot({path:'output/playwright/issue152/browser-draft.png',fullPage:true});
    check(errors.length === 0, 'No browser runtime errors');
    if (checks.length !== 75) throw Error(`Incomplete visual checks ${checks.length}/75`);
    return {completed:checks.length,expected:75,checks};
  } finally { page.off('pageerror', onError); }
}
