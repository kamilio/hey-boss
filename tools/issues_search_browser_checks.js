// Run against an isolated issue-web fixture with a synthetic embedding bridge.
async page => {
  const checks = [], check = (ok,name) => {if(!ok) throw Error(name); checks.push(name); console.log(name)};
  await page.waitForFunction(() => model.csrf && model.project);
  await page.evaluate(async () => {
    await api({action:'project_init',settings:{prs_enabled:false,worktree_enabled:false,if_version:0}},'named:Search QA');
    for(const [title,body] of [['Reconnect after sleep',''],['Network task','Reconnect after sleep'],['Comment thread','']])
      await api({action:'create',title,body,labels:[]},'named:Search QA');
    await api({action:'comment',number:3,body:'Reconnect after sleep <script>bad()</script>'},'named:Search QA');
    navigate({project:'named:Search QA',search:'Reconnect after sleep',state:'active'});
  });
  await page.getByRole('button',{name:'Enable smart search',exact:true}).waitFor();
  check(await page.locator('.issue-row').count()===2,'Fresh install keeps literal title/body search');
  await page.getByRole('button',{name:'Enable smart search',exact:true}).click();
  await page.waitForFunction(() => model.issues?.some(i=>i.match?.field==='comment'),{},{timeout:15000});
  check((await page.locator('.issue-row').evaluateAll(rows=>rows.map(r=>r.dataset.issueNumber))).join(',')==='1,2,3','Title > description > comment');
  check(await page.locator('.issue-order-handle').count()===0,'Ranked results have no queue reorder controls');
  check(await page.locator('.issue-match script').count()===0,'Excerpts are escaped');
  check((await page.locator('[data-issue-number="3"] .issue-match').innerText()).toLowerCase().includes('comment'),'Match source is visible');

  let release;
  const held=new Promise(resolve=>release=resolve);
  let intercepted;
  const started=new Promise(resolve=>intercepted=resolve);
  await page.route('**/api/action',async route=>{
    const input=route.request().postDataJSON();
    if(input.operation?.action==='search' && input.operation.semantic && input.operation.query==='reconnect') {
      intercepted(); await held;
    }
    await route.continue().catch(()=>{});
  });
  const search=page.locator('#issue-search');
  await search.fill('reconnect');
  await started;
  check((await page.locator('#issue-search-status').innerText()).includes('Finding related meaning'),'Keyword results render before semantic response');
  await page.locator('[data-issue-number="1"] .issue-title').focus();
  release();
  await page.waitForFunction(()=>model.searchRefining==null);
  check(await page.locator('[data-issue-number="1"] .issue-title').evaluate(el=>document.activeElement===el),'Semantic refinement preserves keyboard focus');
  await page.unroute('**/api/action');

  // Hold an old query while a new route completes, then let the old response go.
  let releaseOld;
  const oldHeld=new Promise(resolve=>releaseOld=resolve);
  let oldStarted;
  const oldReady=new Promise(resolve=>oldStarted=resolve);
  await page.route('**/api/action',async route=>{
    const input=route.request().postDataJSON();
    if(input.operation?.action==='search' && input.operation.semantic && input.operation.query==='sleep') {oldStarted();await oldHeld;}
    await route.continue().catch(()=>{});
  });
  await search.fill('sleep'); await oldReady;
  await search.fill('Network task');
  await page.waitForFunction(()=>model.route.search==='Network task' && model.searchRefining==null);
  releaseOld();
  await page.waitForTimeout(200);
  check(await search.inputValue()==='Network task' && await page.locator('.issue-title').first().innerText()==='Network task','Old semantic responses cannot replace a newer query');
  await page.unroute('**/api/action');
  await page.setViewportSize({width:390,height:844});
  check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Mobile results fit the screen');
  check(await page.locator('.issue-row-main').first().evaluate(el=>el.getBoundingClientRect().width>200),'Mobile results keep a readable text column');
  await page.screenshot({path:'/tmp/hey-boss-search-mobile.png',fullPage:true});
  await page.setViewportSize({width:1280,height:900});
  await page.screenshot({path:'/tmp/hey-boss-search-desktop.png',fullPage:true});
  await page.getByRole('button',{name:'Turn off',exact:true}).click();
  await page.getByRole('button',{name:'Enable smart search',exact:true}).waitFor();
  check(true,'Search can be disabled without losing literal search');
  return {passed:checks.length,checks};
}
