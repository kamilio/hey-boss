// First visit, rejected pairing and failed offline setup use the real page and service worker.
async (page, fixture) => {
  page.setDefaultTimeout(15000);
  const chat='10000000-0000-4000-8000-000000000000';
  Object.assign(fixture,{pairing:true,paired:false,failWorker:true,chat,revision:1,messages:new Map([[chat,[]]])});
  await page.reload();
  await page.waitForFunction(()=>document.querySelector('#pair').open && !document.querySelector('#offlineNotice').hidden);
  if(await page.locator('footer').isVisible() || await page.locator('#offlineNotice').isVisible() || await page.locator('#history').isVisible()) throw Error('Unpaired page shows unusable conversation controls');
  if(await page.locator('#pair').evaluate(el=>el.matches(':modal'))) throw Error('Pairing obscures the page with a modal backdrop');
  if(await page.locator('#notice').textContent()) throw Error('First visit reports an unrelated error');
  for(const [width,height] of [[1365,900],[390,844],[320,568]]) {
    await page.setViewportSize({width,height});
    const bounds=await page.locator('#pair').boundingBox();
    if(bounds.x<0 || bounds.x+bounds.width>width || bounds.y<0) throw Error('Pairing does not fit the viewport');
    await page.getByText('Where do I find the code?',{exact:true}).click();
    if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth)) throw Error('Pairing instructions overflow the phone');
    await page.getByText('Where do I find the code?',{exact:true}).click();
  }
  await page.getByLabel('Pairing code',{exact:true}).fill('incorrect-code');
  await page.getByRole('button',{name:'Connect',exact:true}).click();
  await page.locator('#pairError').getByText('That code didn’t match.',{exact:false}).waitFor();
  fixture.offline=true;
  await page.getByRole('button',{name:'Connect',exact:true}).click();
  await page.locator('#pairError').getByText('Couldn’t reach your Mac.',{exact:false}).waitFor();
  fixture.offline=false;
  await page.getByLabel('Pairing code',{exact:true}).fill('test-pairing-code');
  await page.getByRole('button',{name:'Connect',exact:true}).click();
  await page.waitForFunction(()=>connected && !document.querySelector('#pair').open);
  if(!await page.locator('footer').isVisible() || !await page.locator('#offlineNotice').isVisible()) throw Error('Connected page hides controls or offline limitation');
  if(await page.locator('#notice').textContent()) throw Error('Pairing left a stale error');
  await page.evaluate(async()=>{
    const register=navigator.serviceWorker.register;
    navigator.serviceWorker.register=()=>Promise.reject(new DOMException('Untrusted certificate','SecurityError'));
    try { await enableOffline(); } finally { navigator.serviceWorker.register=register; }
  });
  if(!(await page.locator('#offlineNotice').textContent()).includes('Trust your Mac’s certificate')) throw Error('Certificate failure has no actionable explanation');
  fixture.failWorker=false;
  await page.getByRole('button',{name:'Retry',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('#offlineNotice').hidden && !!navigator.serviceWorker.controller);
  return 'Conversations pairing checks passed: first visit, phone layout, code help, rejected code, unreachable Mac, offline setup retry';
}
