// Run with playwright-cli against chief_metadata_checks.mjs --moves --serve.
// Screenshots use /tmp/hb-move-visual; remove it after visual inspection.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const onError = error => errors.push(error.message);
  const url = 'http://127.0.0.1:59651/#project=named%3AQueue%20movement%20QA';
  const rows = () => page.locator('#issue-list .issue-row[data-issue-number]');
  const order = () => rows().evaluateAll(elements => elements.map(el => Number(el.dataset.issueNumber)));
  const waitOrder = async expected => {
    await page.waitForFunction(expected => {
      const actual = Array.from(document.querySelectorAll('#issue-list .issue-row[data-issue-number]'), el => Number(el.dataset.issueNumber));
      return JSON.stringify(actual) === JSON.stringify(expected) && !model.orderSaving;
    }, expected);
  };
  const handle = number => page.locator(`.issue-order-handle[data-move-issue="${number}"]`);
  const response = () => page.waitForResponse(r => r.url().endsWith('/api/action') && r.request().postDataJSON()?.operation?.action === 'move');
  page.on('pageerror', onError);
  page.setDefaultTimeout(30000);
  try {
    for (const scheme of ['light','dark']) {
      await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
      for (const width of [1440,768,390,320]) {
        const label = `${scheme}/${width}`;
        await page.setViewportSize({width,height:900});
        await page.goto(url);
        await waitOrder([4,3,1,2]);
        check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No horizontal overflow ${label}`);
        check(await handle(3).isVisible(), `Order handle reachable ${label}`);
        const pending = response();
        await handle(3).press('ArrowUp');
        const receipt = await (await pending).json();
        check(receipt.ok && receipt.changed && receipt.store?.host === 'supervisor', `Keyboard move acknowledged by authority ${label}`);
        await waitOrder([3,4,1,2]);
        check(await handle(3).evaluate(el => el === document.activeElement), `Focus retained ${label}`);
        await page.screenshot({path:`/tmp/hb-move-visual/${scheme}-${width}.png`,fullPage:true});
        await page.reload();
        await waitOrder([3,4,1,2]);
        check(true, `Order persists after reload ${label}`);
        const restore = response();
        await handle(3).press('ArrowDown');
        check((await (await restore).json()).ok, `Keyboard reverse move ${label}`);
        await waitOrder([4,3,1,2]);
      }
    }
    await page.setViewportSize({width:1440,height:900});
    const source = await handle(2).boundingBox();
    const target = await rows().first().boundingBox();
    const pending = response();
    await page.mouse.move(source.x + source.width/2, source.y + source.height/2);
    await page.mouse.down();
    await page.mouse.move(target.x + 80, target.y + 5, {steps:12});
    check(await page.locator('.issue-drop-before').count() === 1, 'Pointer drop indicator visible');
    await page.screenshot({path:'/tmp/hb-move-visual/pointer-drop.png',fullPage:true});
    await page.mouse.up();
    check((await (await pending).json()).store?.host === 'supervisor', 'Pointer move reaches authority');
    await waitOrder([2,4,3,1]);
    await page.reload();
    await waitOrder([2,4,3,1]);
    check(true, 'Pointer move persists after reload');

    // Send an obsolete list version to exercise the real backend's conflict
    // response and the viewer's recovery, without mocking a successful write.
    await page.setViewportSize({width:320,height:900});
    const previous = await order();
    await page.route('**/api/action', async route => {
      const data = route.request().postDataJSON();
      if (data?.operation?.action === 'move') data.operation.if_order_version = 0;
      await route.continue({postData:JSON.stringify(data)});
    });
    const rejected = response();
    await handle(4).press('ArrowUp');
    check((await (await rejected).json()).error?.code === 'conflict', 'Stale queue movement is refused');
    await page.getByText('Issue order changed. Refresh the list and try again.',{exact:true}).waitFor();
    await waitOrder(previous);
    check(await handle(4).evaluate(el => !el.disabled && el === document.activeElement), 'Conflict restores controls and focus');
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Conflict message fits narrow phone');
    await page.screenshot({path:'/tmp/hb-move-visual/stale-phone.png',fullPage:true});
    await page.unroute('**/api/action');
    check(errors.length === 0, 'No browser runtime errors');
    return {completed:checks.length,checks};
  } finally {
    await page.unroute('**/api/action');
    page.off('pageerror', onError);
  }
}
