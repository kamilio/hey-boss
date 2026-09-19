async function mindmapPrChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw new Error(message);
    passed++;
  };
  await page.setViewportSize({width: 1280, height: 900});
  await page.reload();
  const card = page.locator('#mindmap').getByRole('button', {name: '#123', exact: true});
  await card.waitFor();
  check(await page.getByRole('button', {name: 'Fix reconnect', exact: true}).count() === 1, 'Custom PR title is preserved');
  const open = page.getByRole('link', {name: 'Open pull request #123', exact: true});
  check(await open.getAttribute('href') === 'https://github.com/example/repo/pull/123', 'Card CTA retains the original URL');
  check(await open.getAttribute('target') === '_blank' && (await open.getAttribute('rel')).includes('noopener'), 'PR opens safely in another tab');
  await page.context().route('https://github.com/example/repo/pull/123', route => route.fulfill({body: 'PR destination'}));
  const popupPromise = page.waitForEvent('popup');
  await open.click();
  const popup = await popupPromise;
  await popup.waitForLoadState();
  check(popup.url() === 'https://github.com/example/repo/pull/123', 'CTA opens the actual PR destination');
  await popup.close();
  await card.click();
  check(await page.locator('#map-details h2').textContent() === '#123', 'Inspector heading is compact');
  check(await page.locator('#map-details .map-resource-link').getAttribute('href') === 'https://github.com/example/repo/pull/123', 'Inspector still opens the PR');
  check(!(await page.locator('#map-details').innerText()).includes('https://'), 'Inspector does not show redundant URL details');
  await page.getByRole('button', {name: 'Close topic details', exact: true}).click();
  await page.getByRole('button', {name: 'Ship compact mindmap PRs', exact: true}).click();
  check(await page.locator('#map-details .relationships a').textContent() === '#123', 'Relationship label is compact');
  await page.getByRole('button', {name: 'Outline', exact: true}).click();
  check((await page.locator('#outline').innerText()).includes('#123') && !(await page.locator('#outline').innerText()).includes('https://'), 'Outline uses compact labels');
  check(await page.locator('#outline a[href="https://github.com/example/repo/pull/123"]').count() === 1, 'Outline retains Open PR action');
  await page.getByRole('button', {name: 'Map', exact: true}).click();
  for (const query of ['#123', 'https://github.com/example/repo/pull/123']) {
    await page.locator('#search').fill(query);
    await card.waitFor();
    check(await card.count() === 1, `Search finds PR by ${query}`);
  }
  await page.locator('#search').fill('');
  await page.setViewportSize({width: 390, height: 844});
  await page.getByRole('button', {name: 'Close topic details', exact: true}).click();
  await page.getByRole('button', {name: 'Fit', exact: true}).click();
  check(await open.isVisible(), 'Mobile map has a direct PR action');
  const cardBox = await card.boundingBox(), openBox = await open.boundingBox();
  check(openBox.x >= cardBox.x && openBox.x + openBox.width <= cardBox.x + cardBox.width && openBox.y + openBox.height <= cardBox.y + cardBox.height, 'CTA fits inside the compact card on mobile');
  return {passed};
}
