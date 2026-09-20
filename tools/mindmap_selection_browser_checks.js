async function mindmapSelectionChecks(page) {
  let passed = 0;
  const pageErrors = [];
  const onError = error => pageErrors.push(error.message);
  page.on('pageerror', onError);
  const check = (condition, message) => {
    if (!condition) throw new Error(message);
    passed++;
  };
  const settle = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const camera = () => page.locator('.map-world').evaluate(element => element.style.transform);
  for (const width of [1280, 390]) {
    await page.setViewportSize({width, height:900});
    await page.reload();
    await page.locator('[data-map-node]').first().waitFor();
    await page.locator('#search').fill('release');
    await settle();
    // Keep a readable card visible while testing selection below the focus zoom.
    await page.locator('#map-out').click();
    await settle();
    const card = page.getByRole('button', {name:'Autumn release', exact:true});
    const before = await camera();
    await card.click();
    await settle();
    check(await camera() === before, `${width}: selection preserves pan and zoom`);
    check(await page.locator('#map-inspector').isVisible(), `${width}: selection opens details`);
    await page.locator('#close-inspector').click();
    await settle();
    check(await camera() === before, `${width}: closing details preserves pan and zoom`);
    check(await card.evaluate(element => element === document.activeElement), `${width}: closing details returns focus to the card`);
    // A pointer can select the exposed portion of a card at the viewport edge.
    const box = await card.boundingBox();
    const map = await page.locator('#mindmap').boundingBox();
    await page.mouse.move(map.x + map.width / 2, map.y + map.height / 2);
    await page.mouse.wheel(box.x - map.x + box.width / 2, 0);
    await settle();
    const clipped = await card.boundingBox();
    const edgeCamera = await camera();
    await page.mouse.click(map.x + 8, clipped.y + clipped.height / 2);
    await settle();
    check(await page.locator('#map-inspector').isVisible(), `${width}: a partly visible card can be selected`);
    check(await camera() === edgeCamera, `${width}: pointer focus on a partly visible card preserves the camera`);
  }
  await page.setViewportSize({width:1280,height:900});
  await page.reload();
  const beforeSearch = await camera();
  await page.locator('#search').fill('long-planning');
  await settle();
  check(await camera() !== beforeSearch, 'Search still navigates to its matching topic');
  await page.locator('#search').fill('release');
  await settle();
  const card = page.getByRole('button', {name:'Autumn release',exact:true});
  const map = await page.locator('#mindmap').boundingBox();
  await page.mouse.move(map.x + map.width / 2, map.y + map.height / 2);
  await page.mouse.wheel(700, 0);
  await settle();
  await page.locator('#mindmap').focus();
  const offscreen = await camera();
  await page.keyboard.press('Tab');
  await settle();
  check(await card.evaluate(element => element === document.activeElement), 'Tab reaches the off-screen topic');
  check(await camera() !== offscreen, 'Keyboard focus still brings off-screen topics into view');
  const keyboardCamera = await camera();
  await page.keyboard.press('Enter');
  await settle();
  check(await camera() === keyboardCamera, 'Keyboard selection does not recenter an already revealed topic');
  const child = page.locator('[data-select-topic]').first();
  const childTitle = await child.textContent();
  await child.click();
  await settle();
  check(await page.locator('#map-details h2[tabindex]').textContent() === childTitle, 'Child-topic links open the selected child');
  check(await camera() !== keyboardCamera, 'Child-topic links still navigate to the child');
  page.off('pageerror', onError);
  check(pageErrors.length === 0, `No browser errors: ${pageErrors.join('; ')}`);
  return {passed, viewports:2};
}
