// Run on the Workers page of an isolated fixture. All broadcast requests are intercepted.
async page => {
  const base = new URL(page.url()).origin;
  const errors = [], requests = [];
  page.on('pageerror', e => errors.push(e.message));
  let counts = {ok:true,total:3,queued:3,offline:0};
  await page.route('**/api/fleet/broadcast', route => {
    requests.push(route.request().postDataJSON());
    return route.fulfill({json:counts});
  });
  for (const [width, theme] of [[1280,'light'],[390,'dark'],[320,'light']]) {
    await page.setViewportSize({width,height:800});
    await page.emulateMedia({colorScheme:theme});
    await page.goto(base+'/workers');
    const open=page.getByRole('button',{name:'Broadcast',exact:true});
    await open.click();
    const dialog=page.getByRole('dialog',{name:'Broadcast to active agents'});
    const message=dialog.getByLabel('Message');
    await message.fill('First line\nSecond line');
    if (!await dialog.evaluate(d=>d.scrollWidth<=d.clientWidth && d.getBoundingClientRect().right<=innerWidth)) throw Error('Dialog overflow');
    await page.keyboard.press('Escape');
    if (!await open.evaluate(e=>document.activeElement===e)) throw Error('Focus did not return to Broadcast');
    await open.click();
    if (await message.inputValue()!=='First line\nSecond line') throw Error('Draft lost');
    await dialog.getByRole('button',{name:'Send broadcast'}).click();
    await page.getByText(counts.total?'Queued for '+counts.queued+' of '+counts.total+' active agents.':'No active agents on connected machines.',{exact:false}).waitFor();
    counts=width===1280?{ok:true,total:3,queued:1,offline:1}:{ok:true,total:0,queued:0,offline:0};
  }
  if(requests.length!==3 || requests.some(r=>r.text!=='First line\nSecond line'||Object.keys(r).length!==1))throw Error('Unexpected broadcast payload');
  if(errors.length)throw Error(errors.join('\n'));
}
