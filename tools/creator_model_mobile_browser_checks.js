async page => {
  const base='http://127.0.0.1:59735',checks=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const pairing=await(await page.request.get(base+'/fixture-pairing')).json();
  const paired=await page.request.post(base+'/api/pair',{data:pairing});
  check(paired.ok(),'Paired to the production mobile server');
  for(const theme of ['light','dark'])for(const width of [390,768]){
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    await page.setViewportSize({width,height:900});
    await page.goto(base+'/issues#project=named%3ACreator%20model%20QA');
    await page.locator('.issue-row').nth(4).waitFor();
    check((await page.locator('[data-issue-number="1"]').innerText()).includes('Codex · gpt-6-astra'),`${theme}/${width}: model crosses relay`);
    check(await page.locator('[data-issue-number="3"] .issue-meta').evaluate(e=>e.scrollWidth<=e.clientWidth+1),`${theme}/${width}: long model wraps`);
    check(await page.evaluate(()=>document.documentElement.dataset.issueMobile==='true'),`${theme}/${width}: production mobile UI`);
    await page.screenshot({path:`output/playwright/issue134/mobile-${theme}-${width}-list.png`,fullPage:true});
    await page.locator('[data-issue-number="1"] .issue-title').click();
    await page.locator('.detail-meta').waitFor();
    check((await page.locator('.detail-meta').innerText()).includes('Codex · gpt-6-astra'),`${theme}/${width}: detail model`);
    await page.getByText('Issue details',{exact:true}).click();
    check((await page.locator('.origin-author').innerText()).includes('Codex · gpt-6-astra'),`${theme}/${width}: origin model`);
    await page.screenshot({path:`output/playwright/issue134/mobile-${theme}-${width}-detail.png`,fullPage:true});
  }
  return {checks:checks.length};
}
