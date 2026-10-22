// Run against serve_queue_placement_fixture.mjs; screenshots go to a temporary directory.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  page.setDefaultTimeout(15000);
  const native = 'http://127.0.0.1:59701';
  const hash = '#project=named%3AQueue%20placement%20QA&issue=2';
  await page.goto(native + '/' + hash);
  await page.locator('#comments > .activity-group').waitFor();
  await page.evaluate(async () => { await mutate({action:'claim',number:2,force:false}); });
  for (const [surface,base] of [['native',native+'/']]) {
    for (const scheme of ['light','dark']) for (const width of [1440,768,390,320]) {
      const label = `${surface}/${scheme}/${width}`;
      await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
      await page.setViewportSize({width,height:900});
      await page.goto(base+hash);
      await page.reload();
      const group = page.locator('#comments > .activity-group');
      await group.waitFor();
      check(await group.count() === 1, label+': one group');
      check(await group.locator(':scope > summary .activity-count').innerText() === '3 updates',label+': three events grouped');
      check(await group.evaluate(el => !el.open && el.getBoundingClientRect().height <= 44),label+': one compact line');
      check(await page.locator('#comments .issue-activity').count() === 3,label+': complete history retained');
      await page.locator('.issue-discussion').scrollIntoViewIfNeeded();
      await page.screenshot({path:`/tmp/hey-boss-timeline-qa/${surface}-${scheme}-${width}-collapsed.png`});
      const summary = group.locator(':scope > summary');
      await summary.focus();
      await page.keyboard.press('Enter');
      check(await group.evaluate(el=>el.open),label+': keyboard expands group');
      const worker = group.locator('.issue-activity').filter({hasText:'Selection details'});
      await worker.locator(':scope > summary').click();
      await worker.getByText('Selection details',{exact:true}).click();
      check((await worker.innerText()).includes('"required_tags"'),label+': worker diagnostics accessible');
      const created = group.locator('.issue-activity').filter({hasText:'Added to the top of the queue.'});
      await created.locator(':scope > summary').click();
      await created.getByText('Added to the top of the queue.',{exact:true}).waitFor();
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),label+': expanded content fits');
      await page.screenshot({path:`/tmp/hey-boss-timeline-qa/${surface}-${scheme}-${width}-expanded.png`});
      await summary.focus();
      await page.keyboard.press('Space');
      check(await group.evaluate(el=>!el.open),label+': keyboard collapses group');
    }
  }
  // Real API events cross a 30-entry history page; a visible comment separates groups.
  await page.goto(native+'/'+hash);
  await page.locator('#comment-body').fill('The timeline stays readable, with every decision preserved.');
  await page.locator('#comment-submit').click();
  await page.locator('#comments > .comment-card').waitFor();
  await page.evaluate(async () => {
    for(let n=0;n<36;n++) await mutate({action:'edit',number:2,title:'Recovery first — '+n+' '+(n===35?'A very long activity title to test small screens and ensure every full change remains available when expanded. '.repeat(4):''),body:null,add_labels:[],remove_labels:[]});
  });
  for (const [surface,base] of [['native',native+'/']]) {
    await page.goto(base+hash);
    await page.reload();
    const more = page.locator('.history-more');
    await more.waitFor();
    const group = page.locator('#comments > .activity-group');
    check(await group.count()===1,surface+': latest page grouped');
    await group.locator(':scope > summary').click();
    const last = group.locator('.issue-activity').last();
    await last.locator(':scope > summary').click();
    const lastId = await last.getAttribute('data-event-id');
    await page.locator('#comment-body').fill('Draft survives pagination');
    await more.click();
    await page.waitForFunction(()=>!document.querySelector('.history-more'));
    check(await page.locator('#comments > .activity-group').count()===2,surface+': groups merge across page boundary, separated by comment');
    check(await page.locator('#comments .issue-activity').count()===39,surface+': all events retained once');
    check(await page.locator('#comments > .comment-card').count()===1,surface+': comment remains visible');
    check(await page.locator(`[data-event-id="${lastId}"]`).evaluate(el=>el.open&&el.closest('.activity-group').open),surface+': disclosure preserved');
    check(await page.locator('#comment-body').inputValue()==='Draft survives pagination',surface+': draft preserved');
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),surface+': long expanded title fits');
    await page.locator('#comments > .comment-card').scrollIntoViewIfNeeded();
    await page.screenshot({path:`/tmp/hey-boss-timeline-qa/${surface}-paginated.png`});
  }
  check(errors.length===0,'No browser exceptions: '+errors.join('; '));
  return {checks:checks.length,errors};
}
