// Run with playwright-cli run-code against serve_origin_fixture.mjs.
async page => {
  const checks=[],errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  const base='http://127.0.0.1:59675';
  const issue=number=>base+'/#project=named%3AOrigin+QA&issue='+number;
  const fit=async name=>check(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),name+': fits viewport');
  for(const [name,width,height,scheme] of [['desktop-light',1440,1000,'light'],['desktop-dark',1440,1000,'dark'],['phone-light',390,844,'light'],['phone-dark',390,844,'dark'],['small-phone',320,740,'light']]){
    await page.setViewportSize({width,height});await page.emulateMedia({colorScheme:scheme});await page.goto(issue(2),{waitUntil:'domcontentloaded'});
    const origin=page.getByRole('region',{name:'Origin',exact:true});await origin.waitFor();
    const link=origin.getByRole('link',{name:'View creating invocation'});
    check((await link.getAttribute('href')).includes('run=historical-origin&at=173'),name+': preserves exact run and byte offset');
    await origin.getByText('Session details',{exact:true}).click();
    await origin.getByText('create-follow-up',{exact:true}).waitFor();
    await fit(name+' expanded origin');
    check((await link.boundingBox()).height>=44,name+': invocation has a touch target');
    await link.focus();await page.screenshot({path:'output/playwright/issue75/'+name+'-issue.png',fullPage:true});
    await page.keyboard.press('Enter');await page.locator('.chat-message.is-origin').waitFor();
    check(await page.locator('.chat-message.is-origin').evaluate(e=>e.open),name+': creating tool call is expanded');
    check(await page.locator('.chat-message.is-origin').textContent().then(s=>s.includes('hey-boss issue create')),name+': displays actual creating command');
    check(await page.getByRole('button',{name:'Take over',exact:true}).isHidden(),name+': ended run has no takeover action');
    await page.locator('#session-resources').waitFor();await page.locator('#session-resources summary').click();
    check(await page.locator('#session-resources-list a').count()===2,name+': links issue and artifact created by this run');
    await fit(name+' conversation');await page.screenshot({path:'output/playwright/issue75/'+name+'-conversation.png',fullPage:true});
    const artifactLink=page.locator('#session-resources-list a').filter({hasText:'Reconnect investigation'});
    await artifactLink.click();await page.getByRole('heading',{name:'Reconnect investigation',exact:true}).waitFor();
    await page.getByRole('region',{name:'Origin',exact:true}).waitFor();await fit(name+' artifact');
    await page.screenshot({path:'output/playwright/issue75/'+name+'-artifact.png',fullPage:true});
  }
  await page.goto(issue(3),{waitUntil:'domcontentloaded'});await page.getByRole('link',{name:'View creating invocation'}).click();
  await page.locator('.chat-message.is-origin').waitFor();
  check(page.url().includes('run=session%3A11111111'), 'Standalone creation opens its saved session');
  check(await page.locator('#session-issue').isHidden(),'Standalone session has no invented task');
  await page.goto(issue(4),{waitUntil:'domcontentloaded'});await page.getByText('Creation context was not recorded.').waitFor();
  check(await page.getByRole('region',{name:'Origin',exact:true}).getByRole('link').count()===0,'Legacy issue has no invented origin links');
  await page.goto(issue(5),{waitUntil:'domcontentloaded'});await page.getByRole('link',{name:'View creating invocation'}).click();
  await page.getByRole('alert').filter({hasText:'This device is disconnected'}).waitFor();
  check(await page.locator('.chat-message').count()===0,'Disconnected origin never falls back to another session');
  const refused=await page.request.get(base+'/api/fleet/conversation?host=local&run=unrelated');
  check(!refused.ok(),'Unknown historical run is rejected');
  check(errors.length===0,'No JavaScript exceptions: '+errors.join('; '));
  return {checks:checks.length,passed:true};
}
