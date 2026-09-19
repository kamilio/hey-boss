async function mindmapControlsChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw new Error(message);
    passed++;
  };
  const geometry = () => page.evaluate(() => {
    const rect = selector => document.querySelector(selector).getBoundingClientRect();
    const inspector = rect('#map-inspector');
    const overlap = box => Math.max(0, Math.min(box.right, inspector.right) - Math.max(box.left, inspector.left)) * Math.max(0, Math.min(box.bottom, inspector.bottom) - Math.max(box.top, inspector.top));
    return Object.fromEntries(['#map-overview', '#map-fit', '#map-in', '#map-out'].map(selector => {
      const box = rect(selector);
      return [selector, {
        overlap: document.querySelector('#map-inspector').hidden ? 0 : overlap(box),
        receivesPointer: !!document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2)?.closest(selector),
      }];
    }));
  });
  for (const viewport of [
    {width:1600,height:1400}, {width:900,height:1200},
    {width:820,height:1200}, {width:701,height:1200},
    {width:700,height:900}, {width:390,height:844}, {width:320,height:740},
  ]) {
    await page.setViewportSize(viewport);
    await page.reload();
    await page.locator('#search').fill('release');
    await page.getByRole('button', {name:'Autumn release',exact:true}).click();
    check(!await page.locator('#map-inspector').getByText('Topic details', {exact:true}).count(), `${viewport.width}: details omit the generic title`);
    check(await page.getByRole('button', {name:'Close topic details',exact:true}).isVisible(), `${viewport.width}: details retain an accessible close button`);
    check(await page.locator('#close-inspector').evaluate(button => {
      const heading = button.parentElement.getBoundingClientRect();
      const box = button.getBoundingClientRect();
      return box.x > heading.x + heading.width / 2;
    }), `${viewport.width}: close button stays on the right`);
    await page.locator('#mindmap').scrollIntoViewIfNeeded();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    for (const [selector, result] of Object.entries(await geometry())) {
      check(result.overlap === 0, `${viewport.width}: inspector does not cover ${selector}`);
      check(result.receivesPointer, `${viewport.width}: ${selector} receives a real pointer`);
    }
    const zoom = await page.locator('#map-zoom').textContent();
    await page.locator('#map-in').click();
    await page.waitForFunction(value => document.querySelector('#map-zoom').textContent !== value, zoom);
    check(await page.locator('#map-zoom').textContent() !== zoom, `${viewport.width}: zoom responds while details are open`);
    await page.locator('#map-out').click();
    await page.locator('#map-fit').click();
    await page.locator('#map-overview').click();
    check(await page.locator('#map-inspector').isVisible(), `${viewport.width}: map navigation preserves topic details`);
    check(await page.locator('#map-details').evaluate(element => element.clientHeight > 60), `${viewport.width}: details retain a readable scroll area`);
    await page.locator('#close-inspector').click();
    for (const [selector, result] of Object.entries(await geometry()))
      check(result.receivesPointer, `${viewport.width}: closing details preserves ${selector}`);
  }
  return {passed, viewports:7};
}
