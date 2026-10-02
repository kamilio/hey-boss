// Run with playwright-cli run-code --filename against serve_archive_fixture.mjs.
async page => {
  const checks=[],errors=[];
  const check=(ok,label)=>{if(!ok)throw Error(label);checks.push(label);};
  page.on('pageerror',e=>errors.push(e.message));
  page.setDefaultTimeout(20000);
  await page.route('**/api/inbox',r=>r.fulfill({json:{ok:true,tasks:[],unread_count:0}}));
  const phone='http://127.0.0.1:52062';
  const {code}=await(await page.request.get(phone+'/fixture-pairing')).json();
  check((await page.request.post(phone+'/api/pair',{data:{code}})).ok(),'Phone pairing');
  for(const [surface,base] of [['desktop','http://127.0.0.1:59662/'],['phone',phone+'/issues']]) {
    const detail=async n=>{await page.goto(base+'#project=named%3AArchive%20QA&issue='+n);await page.locator('.issue-description').waitFor();await page.locator('.issue-activity').first().waitFor();};
    await detail(1);
    check((await page.locator('.issue-description').innerText()).includes('cold-search-needle 🦀'),surface+': cold body renders');
    check(await page.locator('.issue-description strong').innerText()==='Formatting survives.',surface+': rich body unchanged');
    const ids=await page.locator('#comments [data-event-id]').evaluateAll(nodes=>nodes.map(n=>n.dataset.eventId));
    await page.locator('#comment-body').fill('Unsent draft across archive history pagination');
    await page.getByRole('button',{name:'Load earlier activity'}).click();
    await page.locator('.history-more').waitFor({state:'hidden'});
    const loaded=await page.locator('#comments [data-event-id]').evaluateAll(nodes=>nodes.map(n=>n.dataset.eventId));
    check(loaded.length>ids.length&&new Set(loaded).size===loaded.length,surface+': cold timeline paginates without duplicates');
    check(await page.locator('#comments .comment-card').count()===1,surface+': archived comment visible once');
    check(await page.locator('#comments .comment-body strong').innerText()==='survives',surface+': rich comment unchanged');
    check(await page.locator('#comment-body').inputValue()==='Unsent draft across archive history pagination',surface+': history preserves draft');
    for(const theme of ['light','dark'])for(const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
      await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);
      check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),surface+'/'+theme+'/'+width+': no horizontal overflow');

    }
    await page.locator('#comment-body').fill('');
    await page.setViewportSize({width:1100,height:900});
    await page.goto(base+'#project=named%3AArchive%20QA&state=closed');
    await page.getByRole('searchbox',{name:'Search issues'}).fill('cold-search-needle');
    await page.waitForFunction(()=>model.issues?.length===3);
    check((await page.locator('main').innerText()).includes('Closed archive conversation 1'),surface+': closed search finds cold body');
    await detail(3);
    check(await page.locator('#comment-form').count()===0,surface+': deleted remains read only');
    check((await page.locator('#comments').innerText()).includes('Saved discussion'),surface+': deleted history retained');
  }
  await page.goto('http://127.0.0.1:59662/#project=named%3AArchive%20QA&issue=1');
  await page.locator('#comment-body').waitFor();await page.locator('#comment-body').fill('Draft survives reopening');
  await page.getByRole('button',{name:'Reopen issue',exact:true}).click();
  await page.locator('.state-pill.open').waitFor();
  check(await page.locator('#comment-body').inputValue()==='Draft survives reopening','Reopen preserves draft');
  check((await page.locator('.issue-description').innerText()).includes('cold-search-needle'),'Reopen preserves body');
  await page.locator('#comment-body').fill('New comment after restoration');await page.locator('#comment-submit').click();
  await page.waitForFunction(()=>document.querySelector('#comments')?.textContent.includes('New comment after restoration'));
  check(await page.locator('#comments .comment-card').count()===2,'Reopened issue accepts new comments');
  await page.goto(phone+'/issues#project=named%3AArchive%20QA&issue=3');
  await page.getByRole('button',{name:'Restore issue',exact:true}).first().click();
  await page.locator('#comment-form').waitFor();
  check((await page.locator('.issue-description').innerText()).includes('cold-search-needle'),'Phone restore preserves body');
  check((await page.locator('#comments').innerText()).includes('Saved discussion'),'Phone restore preserves comments');
  check(errors.length===0,'No browser JavaScript errors: '+errors.join('; '));
  return {count:checks.length,checks};
}
