// Run with a Playwright page on an isolated artifact fixture containing html-render.md.
async page => {
  const assert=(ok,message)=>{if(!ok)throw Error(message);};
  await page.locator('.html-preview[data-state="ready"]').first().waitFor();
  const figure=page.locator('.html-preview').first(),frame=figure.frameLocator('iframe');
  assert(await page.locator('.html-preview').count()===1,'Only an explicit render fence runs');
  assert(await frame.locator('h2').innerText()==='Small changes add up.','Authored HTML renders');
  await frame.locator('#counter').click();
  assert(await frame.locator('#counter').innerText()==='1','Scripts respond to clicks');
  const isolation=await frame.locator('body').evaluate(()=>({parent:(()=>{try{return !!parent.document.body}catch{return false}})(),storage:(()=>{try{return !!localStorage}catch{return false}})()}));
  assert(!isolation.parent&&!isolation.storage,'Preview cannot read host DOM or storage');
  const initial=await figure.locator('iframe').evaluate(n=>n.clientHeight);
  await frame.locator('#extra').evaluate(n=>{n.style.height='2600px';});
  await frame.locator('#toggle').click();
  await page.waitForFunction(()=>document.querySelector('.html-preview iframe').clientHeight>2600);
  assert(await frame.locator('html').evaluate(n=>getComputedStyle(n).overflowY==='hidden'),'Preview uses the page scrollbar');
  assert(await figure.locator('iframe').evaluate(n=>n.getAttribute('scrolling')==='no'),'Frame never adds a nested scrollbar');
  await frame.locator('#toggle').click();
  await page.waitForFunction(height=>document.querySelector('.html-preview iframe').clientHeight<=height+2,initial);
  await figure.getByRole('button',{name:'Expand',exact:true}).click();
  assert(await page.locator('.html-preview-stage:modal').count()===1,'Expanded view opens');
  assert(await frame.locator('#counter').innerText()==='1','Expanding preserves widget state');
  await frame.locator('#counter').focus();await page.keyboard.press('Escape');
  await page.waitForFunction(()=>!document.querySelector('.html-preview-stage:modal'));
  assert(await figure.getByRole('button',{name:'Expand',exact:true}).evaluate(n=>n===document.activeElement),'Escape restores focus');
  assert(await frame.locator('#counter').innerText()==='1','Closing preserves widget state');
  await figure.locator('summary').click();
  assert((await figure.locator('pre').textContent()).includes('window.previewLoaded=true;'),'Exact source remains available');
  assert(!await page.evaluate(()=>window.exampleMustNotRun),'Code examples never execute');
  for(const appearance of ['dark','light']){
    await page.emulateMedia({colorScheme:appearance});
    await frame.locator('body').evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    const foreground=await figure.evaluate(n=>getComputedStyle(n).color);
    await frame.locator('body').evaluate(()=>new Promise(resolve=>setTimeout(resolve,50)));
    assert(await frame.locator('html').evaluate(n=>getComputedStyle(n).getPropertyValue('--foreground').trim())===foreground,'Live theme matches surrounding text');
    for(const width of [320,390,728,1280]){
      await page.setViewportSize({width,height:900});
      const height=await frame.locator('body').evaluate(n=>Math.ceil(n.scrollHeight));
      await page.waitForFunction(height=>document.querySelector('.html-preview iframe').clientHeight>=height,height);
      assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),appearance+' '+width+' stays within the viewport');
    }
  }
  console.log('PASS HTML interaction, sandbox, resizing, theme, mobile widths, source and expansion');
};
