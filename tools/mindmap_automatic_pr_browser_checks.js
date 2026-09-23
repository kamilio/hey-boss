async function mindmapAutomaticPrChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw new Error(message);
    passed++;
  };
  const errors = [];
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  // Run on a map containing an issue and its attached automatic PR #123.
  for (const width of [1280, 390]) {
    await page.setViewportSize({width, height:900});
    await page.reload();
    const card = page.getByRole('button', {name:'#123', exact:true});
    await card.waitFor();
    await card.click();
    const details = page.locator('#map-details');
    check((await details.innerText()).includes('Shown automatically from an issue attachment'), `${width}: automatic PR explanation`);
    check((await details.innerText()).includes('hey-boss mm pr URL'), `${width}: explicit node workaround`);
    check(await details.locator('#node-attachments, #node-artifacts').count() === 0, `${width}: no stored-node attachment controls`);
    check(await details.getByRole('link', {name:'Open pull request ↗',exact:true}).getAttribute('href') === 'https://github.com/example/repo/pull/123', `${width}: original PR destination`);
    check(await details.locator('.relationships a').count() === 1, `${width}: automatic issue relationship`);
    check(await page.locator('body').evaluate(el => el.scrollWidth <= innerWidth), `${width}: no page overflow`);
    for (const theme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme:theme});
      await page.screenshot({path:`output/playwright/issue126/automatic-details-${theme}-${width}.png`});
    }
    await page.getByRole('button', {name:'Close topic details',exact:true}).click();
    await page.getByRole('button', {name:'Outline',exact:true}).click();
    check((await page.locator('#outline').innerText()).includes('#123'), `${width}: PR outline label`);
    check(await page.locator('#outline a[href="https://github.com/example/repo/pull/123"]').count() === 1, `${width}: PR outline destination`);
    await page.screenshot({path:`output/playwright/issue126/automatic-outline-${width}.png`});
    await page.getByRole('button', {name:'Map',exact:true}).click();
    await page.getByRole('searchbox', {name:'Search mindmap'}).fill('https://github.com/example/repo/pull/123');
    check(await card.count() === 1, `${width}: PR searchable by URL`);
    await page.getByRole('searchbox', {name:'Search mindmap'}).fill('');
  }
  page.off('pageerror', onError);
  check(errors.length === 0, `No page errors: ${errors.join('; ')}`);
  return {passed, viewports:2};
}

async function mindmapExplicitPrChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw new Error(message);
    passed++;
  };
  // After mm pr URL --under issue:1 --title 'Short label' on the same fixture.
  for (const width of [1280, 390]) {
    await page.setViewportSize({width, height:900});
    await page.reload();
    await page.getByRole('button', {name:'Short label',exact:true}).click();
    const details = page.locator('#map-details');
    check(await details.locator('.automatic-pr-note').count() === 0, `${width}: stored PR omits automatic note`);
    check(await details.getByRole('button', {name:'Attach files',exact:true}).count() === 1, `${width}: stored PR file controls`);
    check(await details.getByRole('link', {name:'Create artifact',exact:true}).count() === 1, `${width}: stored PR artifact controls`);
    check(await details.locator('.relationships a').count() === 1, `${width}: stored PR relationship`);
    await page.waitForFunction(() => document.querySelector('#node-attachments .attachment-status')?.textContent !== 'Loading attachments…');
    check(await details.getByRole('alert').count() === 0, `${width}: no attachment errors`);
    for (const theme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme:theme});
      await page.screenshot({path:`output/playwright/issue126/explicit-details-${theme}-${width}.png`});
    }
    await page.getByRole('button', {name:'Close topic details',exact:true}).click();
  }
  return {passed, viewports:2};
}
