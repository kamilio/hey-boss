// playwright-cli run-code --filename, with chief_metadata_checks.mjs --serve.
async page => {
  const checks = [], errors = [];
  const check = (value, name) => { if (!value) throw Error(name); checks.push(name); };
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  try {
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme});
      for (const width of [1440, 768, 390, 320]) {
        await page.setViewportSize({width, height:900});
        for (const number of [1, 2]) {
          await page.goto(`http://127.0.0.1:59651/#project=named%3AChief%20metadata%20QA&issue=${number}`);
          await page.waitForFunction(number => model.detail?.issue.number === number, number);
          check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No overflow ${colorScheme} ${width} #${number}`);
          check(await page.locator('[data-edit]').isVisible(), `Edit reachable ${colorScheme} ${width} #${number}`);
          check(await page.evaluate(number => model.detail.issue.labels.includes('qualified') && model.detail.issue.labels.includes('reviewed') && !model.detail.issue.labels.includes('rework needed') && model.detail.issue.state === (number === 1 ? 'closed' : 'open') && model.detail.issue.assignee === (number === 1 ? null : 'codex:active-worker'), number), `Saved labels, lifecycle and ownership ${colorScheme} ${width} #${number}`);
          if ([1440,320].includes(width)) await page.screenshot({path:`output/playwright/issue151/${colorScheme}-${width}-${number}.png`, fullPage:true});
        }
      }
      await page.locator('[data-edit]').click();
      await page.locator('#editor-subject').fill('Keep this unsaved draft');
      await page.locator('#editor-submit').click();
      await page.locator('#editor-error').waitFor({state:'visible'});
      check((await page.locator('#editor-error').innerText()).includes('Changes were not saved'), `Offline editor retains explicit denial ${colorScheme}`);
      check(await page.locator('#editor-subject').inputValue() === 'Keep this unsaved draft', `Rejected draft retained ${colorScheme}`);
      check(await page.locator('#editor-error').evaluate(el => el.scrollWidth <= el.clientWidth), `Denied edit message fits phone ${colorScheme}`);
      await page.screenshot({path:`output/playwright/issue151/${colorScheme}-denied.png`});
      await page.locator('#editor-cancel').click();
      check(await page.locator('[data-edit]').evaluate(el => el === document.activeElement), `Cancel restores focus ${colorScheme}`);
    }
    check(errors.length === 0, 'No browser runtime errors');
    if (checks.length !== 57) throw Error(`Incomplete visual graph ${checks.length}/57`);
    return {completed:checks.length, expected:57, checks};
  } finally { page.off('pageerror', onError); }
}
