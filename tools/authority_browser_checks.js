// Run through playwright-cli against serve_authority_fixture.mjs.
async function authorityBrowserChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw Error(message);
    passed++;
  };
  for (const colorScheme of ['light', 'dark']) {
    await page.emulateMedia({colorScheme});
    for (const width of [1600, 900, 820, 701, 700, 390, 320]) {
      await page.setViewportSize({width, height: width > 700 ? 1000 : 844});
      await page.reload();
      await page.getByRole('button', {name:'Autumn release', exact:true}).waitFor();
      check(!await page.locator('#error').isVisible(), `${width}: authoritative map loaded`);
      check(await page.locator('body').evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${width}: no horizontal overflow`);
      await page.getByRole('searchbox', {name:'Search mindmap'}).fill('release');
      await page.getByRole('button', {name:'Autumn release', exact:true}).click();
      const details = page.locator('#map-details');
      await details.getByText('A shared authoritative map, available from every connected device.', {exact:true}).waitFor();
      check(await details.getByRole('heading', {name:'Autumn release', exact:true}).isVisible(), `${width}: authoritative details`);
      await details.getByRole('button', {name:'Design review', exact:true}).click();
      await details.getByRole('heading', {name:'Design review', exact:true}).waitFor();
      check(await details.getByText('Review keyboard access, layout, and clear offline recovery.', {exact:true}).isVisible(), `${width}: child navigation`);
      await details.locator('.attachment-status').filter({hasText:'Loading attachments'}).waitFor({state:'hidden'});
      check(!await details.locator('.attachment-error').isVisible(), `${width}: authoritative topic attachments loaded`);
      check(await details.getByRole('button', {name:'design.txt',exact:true}).isVisible(), `${width}: authoritative file visible`);
      check(await details.getByRole('link', {name:'Review notes',exact:true}).isVisible(), `${width}: linked artifact visible`);
      const close = page.getByRole('button', {name:'Close topic details', exact:true});
      check(await close.evaluate(button => {
        const r = button.getBoundingClientRect();
        return r.top >= 0 && r.bottom <= innerHeight && document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)?.closest('#close-inspector');
      }), `${width}: close button reachable`);
      await close.focus();
      await page.keyboard.press('Enter');
      check(!await page.locator('#map-inspector').isVisible(), `${width}: keyboard closes details`);
      await page.locator('#mindmap').scrollIntoViewIfNeeded();
      const zoom = await page.locator('#map-zoom').textContent();
      await page.getByRole('button', {name:'Zoom in', exact:true}).click();
      await page.waitForFunction(value => document.querySelector('#map-zoom').textContent !== value, zoom);
      check(await page.locator('#map-zoom').textContent() !== zoom, `${width}: zoom restored`);
      await page.getByRole('button', {name:'Zoom out', exact:true}).click();
      await page.getByRole('button', {name:'Fit', exact:true}).click();
      await page.getByRole('button', {name:'Center map from overview', exact:true}).click();
      check(!await page.locator('#error').isVisible(), `${width}: navigation preserves map`);
      await page.getByRole('button', {name:'Outline', exact:true}).click();
      check(await page.locator('#outline').isVisible(), `${width}: outline is available`);
      await page.getByRole('button', {name:'Map', exact:true}).click();
      await page.getByRole('searchbox', {name:'Search mindmap'}).fill('');
      if ([1600, 390].includes(width)) {
        await page.evaluate(() => scrollTo(0, 0));
        await page.screenshot({path:`output/playwright/issue145/${colorScheme}-${width}.png`});
      }
    }
  }
  return {completed:true, passed, viewports:7, colorSchemes:2};
}

// Stop only the fixture supervisor before invoking this check.
async function authorityOfflineChecks(page) {
  await page.getByRole('button', {name:'Refresh', exact:true}).click();
  const error = page.locator('#error');
  await error.waitFor({state:'visible'});
  const message = await error.innerText();
  for (const text of ['existing supervisor connection', 'No local fallback', 'Refresh to retry', 'last loaded outline']) {
    if (!message.includes(text)) throw Error(`Offline recovery is missing: ${text}`);
  }
  if (!await page.getByRole('button', {name:'Autumn release', exact:true}).isVisible())
    throw Error('Disconnected viewer lost its last loaded map');
  await page.screenshot({path:'output/playwright/issue145/offline-phone.png'});
  return {completed:true, passed:5, state:'offline'};
}

// Restart the fixture supervisor before invoking this check.
async function authorityRecoveryChecks(page) {
  await page.getByRole('button', {name:'Refresh', exact:true}).click();
  await page.locator('#error').waitFor({state:'hidden'});
  if (!(await page.locator('#connection').innerText()).includes('Connected'))
    throw Error('Viewer did not recover its live connection');
  await page.getByRole('button', {name:'Autumn release', exact:true}).click();
  await page.locator('#map-details').getByRole('heading', {name:'Autumn release', exact:true}).waitFor();
  await page.locator('#map-details .attachment-status').filter({hasText:'Loading attachments'}).waitFor({state:'hidden'});
  if (await page.locator('#map-details .attachment-error').isVisible())
    throw Error('Recovered topic attachments failed to load');
  await page.screenshot({path:'output/playwright/issue145/recovered-phone.png'});
  return {completed:true, passed:3, state:'reconnected'};
}

async function authorityResourceChecks(page) {
  const details = page.locator('#map-details');
  await details.getByRole('button', {name:'Design review',exact:true}).click();
  await details.getByRole('button', {name:'design.txt',exact:true}).waitFor();
  const pending = page.waitForEvent('download');
  await details.getByRole('button', {name:'design.txt',exact:true}).click();
  const download = await pending;
  if (download.suggestedFilename() !== 'design.txt' || await download.failure())
    throw Error('Authoritative attachment download failed');
  await download.saveAs('output/playwright/issue145/downloaded-design.txt');
  await page.screenshot({path:'output/playwright/issue145/topic-resources-phone.png'});
  await details.getByRole('link', {name:'Review notes',exact:true}).click();
  await page.locator('#artifact-reading').getByText('Verify attached resources through the same supervisor connection.', {exact:true}).waitFor();
  if (await page.locator('#artifact-error').isVisible())
    throw Error('Linked authoritative document failed to load');
  await page.locator('#artifact-attachments .attachment-status').filter({hasText:'Loading attachments'}).waitFor({state:'hidden'});
  if (await page.locator('#artifact-attachments .attachment-error').isVisible())
    throw Error('Linked document attachments failed to load');
  await page.screenshot({path:'output/playwright/issue145/linked-document-phone.png'});
  return {completed:true, passed:4, state:'resources'};
}
