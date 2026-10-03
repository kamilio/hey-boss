// Run against chief_metadata_checks.mjs --lifecycle --serve using playwright-cli.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  page.setDefaultTimeout(30000);
  const open = async number => {
    await page.goto(`http://127.0.0.1:59651/#project=named%3AChief%20metadata%20QA&issue=${number}`);
    await page.waitForFunction(n => typeof model !== 'undefined' && model.detail?.issue.number === n, number);
  };
  try {
    for (const scheme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
      for (const width of [1440,768,390,320]) {
        await page.setViewportSize({width,height:900});
        for (const [number,state,owner] of [[5,'ready','human:boss'],[6,'closed',null],[7,'ready','watcher:github'],[2,'open','codex:active-worker']]) {
          await open(number);
          check(await page.locator(`.state-pill.${state}`).first().isVisible(), `Lifecycle visible ${scheme}/${width}/#${number}`);
          check(await page.evaluate(owner => model.detail.issue.assignee === owner, owner), `Ownership retained ${scheme}/${width}/#${number}`);
          check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No overflow ${scheme}/${width}/#${number}`);
          if (number === 6) check(await page.getByText('Implementation and installation verified.',{exact:true}).isVisible(), `Closing comment visible ${scheme}/${width}`);
          if ([1440,390].includes(width)) await page.screenshot({path:`/tmp/hb-lifecycle-visual/${scheme}-${width}-${number}.png`,fullPage:true});
        }
      }
    }
    await open(8);
    const ready = page.getByRole('button',{name:'PR ready',exact:true});
    const response = page.waitForResponse(r => r.url().endsWith('/api/action') && r.request().postDataJSON()?.operation?.action === 'ready');
    await ready.press('Enter');
    const receipt = await (await response).json();
    check(receipt.issue?.state === 'ready' && receipt.store?.host === 'supervisor', 'Browser receives the authoritative Ready receipt');
    await page.locator('.state-pill.ready').first().waitFor();
    check(await page.evaluate(() => model.detail.issue.assignee === 'human:boss'), 'Keyboard Ready hands off through the companion tunnel');
    await page.waitForFunction(async () => (await api({action:'view',number:8})).issue.state === 'ready');
    await page.reload();
    await page.locator('.state-pill.ready').first().waitFor();
    check(await page.evaluate(() => model.detail.issue.state === 'ready'), 'Ready survives replica reload');
    await page.screenshot({path:'/tmp/hb-lifecycle-visual/keyboard-ready-phone.png',fullPage:true});
    check(errors.length === 0, 'No browser runtime errors');
    return {completed:checks.length,checks};
  } finally { page.off('pageerror',onError); }
}
