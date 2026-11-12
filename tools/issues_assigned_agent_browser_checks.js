// Run against the assigned-agent web-fixture scenario with playwright-cli run-code.
async (page) => {
  const checks=[],errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const base='http://127.0.0.1:59668/#project=named%3AAssignment+QA';
  const badge=n=>page.locator(`[data-issue-number="${n}"] .assignment-badge`);
  const trace=n=>page.locator(`#assignment-card-${n} .assignment-card-primary`);
  const list=async()=>{await page.goto(base);await badge(1).waitFor();};
  for(const [name,width,height,scheme] of [['desktop-light',1440,1000,'light'],['desktop-dark',1440,1000,'dark'],['mobile-light',390,844,'light'],['mobile-dark',390,844,'dark']]){
    await page.setViewportSize({width,height});await page.emulateMedia({colorScheme:scheme});await list();
    check(await trace(2).count()===0,name+': Boss has no trace arrow');
    check(await trace(3).count()===0,name+': unassigned has no trace arrow');
    await badge(1).press('ArrowDown');
    check(await trace(1).evaluate(el=>{const r=el.getBoundingClientRect();return el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2));}),name+': arrow receives pointer above row overlay');
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),name+': list fits viewport');
    if(width===390){const r=await trace(1).boundingBox();check(r.width>=44&&r.height>=44,name+': touch target is 44px');}
    await trace(1).focus();
    await page.setViewportSize({width,height:height-1});
    await page.screenshot({path:'output/playwright/issue68/'+name+'.png'});
    check(await trace(1).evaluate(el=>document.activeElement===el),name+': conversation link keeps keyboard focus');
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>location.hash.includes('run=assigned-run'));
    check(page.url().includes('host=local'),name+': opens exact owning device and run');
    await page.getByText('The correct assigned session is open.',{exact:true}).waitFor();
    check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),name+': conversation fits viewport');
    await page.screenshot({path:'output/playwright/issue68/'+name+'-conversation.png',fullPage:true});
    await page.goBack();await badge(1).waitFor();
    check(await page.evaluate(() => model.route.owner)==='all',name+': returning preserves list without applying filter');
  }
  await badge(1).press('ArrowDown');
  await page.locator('#assignment-card-1 a').filter({hasText:'Filter by'}).click();
  await page.waitForFunction(()=>model.route.owner==='codex:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'&&model.issues.length===1);
  check(true,'Badge still filters by the assigned session');
  await list();await badge(4).press('ArrowDown');await trace(4).click();
  await page.getByText('Conversation unavailable',{exact:true}).waitFor();
  check((await page.locator('#session-status').innerText()).includes('no longer has a saved Codex session'),'Missing trace explains availability without opening someone else');
  await page.goto('http://127.0.0.1:59668/agents/session#project=named%3AAssignment+QA&issue=5&agent=codex%3Aaaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee');
  await page.waitForFunction(()=>location.hash.includes('run=remote-run'));
  check(page.url().includes('host=devbox'),'Disconnected remote assignment opens its exact last-known trace');
  check(errors.length===0,'No JavaScript runtime errors');
  return {passed:checks.length,checks};
}
