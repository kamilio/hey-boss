// Run with playwright-cli run-code --filename on the local artifacts page.
// All artifact writes are intercepted; no comments are added to the live document.
async page => {
  const checks=[],errors=[],requests=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.on('pageerror',e=>errors.push(e.message));
  const id='a-11111111111111111111111111111111';
  const artifact={id,title:'Comment keyboard checks',version:1,updated_at:new Date().toISOString(),body:'',body_html:Array.from({length:40},(_,i)=>`<p>Passage ${i}: Keep the surrounding document in view while writing a comment.</p>`).join('')};
  const comments=[];
  let fail=false;
  const route=async route=>{
    const payload=route.request().postDataJSON();
    const operation=payload.operation;
    if(operation.action==='attachment')return route.fulfill({json:{ok:true,attachments:[]}});
    if(operation.action!=='artifact')return route.continue();
    const op=operation.operation;
    if(op.command==='comment'){
      requests.push(payload);
      await new Promise(resolve=>setTimeout(resolve,200));
      if(fail)return route.fulfill({status:503,json:{error:'Temporarily offline; retry your comment.'}});
      comments.push({...op,id:comments.length+1,author:'boss',body_html:`<p>${op.body}</p>`,created_at:new Date().toISOString()});
    }
    return route.fulfill({json:{ok:true,artifact,comments,backlinks:[],artifacts:[artifact]}});
  };
  await page.route('**/api/action',route);
  const url=new URL(page.url());url.hash=new URLSearchParams({project:new URLSearchParams(url.hash.slice(1)).get('project'),artifact:id});
  const select=async position=>{
    await page.locator('#artifact-reading > p').nth(25).evaluate((p,position)=>{
      p.scrollIntoView({block:position});
      const range=document.createRange();range.selectNodeContents(p);
      getSelection().removeAllRanges();getSelection().addRange(range);
      p.dispatchEvent(new PointerEvent('pointerup',{bubbles:true}));
    },position);
    await page.locator('#artifact-selection-comment').waitFor({state:'visible'});
  };
  for(const width of [1440,390]){
    await page.setViewportSize({width,height:844});await page.goto(url.href);await page.reload();
    await page.locator('#artifact-reading > p').first().waitFor();
    for(const position of ['center','end']){
      await select(position);
      const before=await page.evaluate(()=>({scroll:scrollY,width:document.querySelector('#artifact-reading').offsetWidth}));
      await page.locator('#artifact-selection-comment').click();
      check(await page.locator('#artifact-reading #artifact-comment').evaluate(e=>e===document.activeElement),`${width}: selection focuses inline composer`);
      check(!await page.locator('#artifact-quote').isVisible(),'Inline composer does not repeat the selected text');
      check(await page.evaluate(()=>CSS.highlights.get('artifact-comment-selection')?.size===1),'Passage stays highlighted after the textarea receives focus');
      check(await page.evaluate(before=>Math.abs(scrollY-before.scroll)<2&&document.querySelector('#artifact-reading').offsetWidth===before.width,before),`${width}: opening preserves scroll and document width (${position})`);
      const field=page.locator('#artifact-comment');
      check(await field.evaluate(e=>{const r=e.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight&&document.documentElement.scrollWidth<=innerWidth}),`${width}: input remains inside viewport`);
      await field.fill('First line');await field.press('Enter');await field.press('a');
      check(await field.inputValue()==='First line\na','Enter adds a newline');
      const count=requests.length;
      await field.dispatchEvent('keydown',{key:'Enter',shiftKey:true,isComposing:true});
      check(requests.length===count,'IME composition does not submit');
      await field.press('Escape');
      check(await page.evaluate(()=>!CSS.highlights.has('artifact-comment-selection')),'Closing clears the passage highlight');
      check(await field.inputValue()==='First line\na','Escape preserves draft');
      check(await page.evaluate(before=>Math.abs(scrollY-before.scroll)<2,before),'Escape preserves scroll');
      await select(position);await page.locator('#artifact-selection-comment').click();
      fail=true;await field.press('Shift+Enter');await field.press('Shift+Enter');
      await page.locator('.artifact-comment-error:not([hidden])').waitFor();
      check(requests.length===count+1,'Repeated shortcut sends once while pending');
      check(await field.inputValue()==='First line\na','Failed send preserves draft inline');
      fail=false;await field.press('Shift+Enter');
      await page.locator('#artifact-reading #artifact-comment-form').waitFor({state:'detached'});
      check(requests.at(-1).request_id===requests.at(-2).request_id,'Retry reuses pending request ID');
      check(requests.at(-1).operation.operation.quote.startsWith('Passage 25:'),'Saved comment retains selected quote');
      check(await page.evaluate(before=>Math.abs(scrollY-before.scroll)<2,before),'Submitting preserves reading position');
    }
    await page.locator('#artifact-comments-toggle').click();
    check(await page.locator('#artifact-comment').evaluate(e=>e===document.activeElement),'Comments button focuses composer');
    const reply=page.locator('.artifact-reply').first();await reply.locator('summary').click();
    await page.waitForFunction(()=>document.activeElement?.getAttribute('aria-label')==='Reply to comment');
    await reply.locator('textarea').fill('Reply using the keyboard');await reply.locator('textarea').press('Shift+Enter');
    await page.waitForFunction(()=>document.querySelector('#artifact-status').textContent==='Reply saved');
    check(requests.at(-1).operation.operation.parent===1,'Reply shortcut keeps the parent thread');
  }
  await page.locator('#artifact-comments-close').click();
  await select('center');await page.locator('#artifact-selection-comment').click();
  await page.locator('#artifact-comment').fill('Keep this draft while the document changes');
  await page.locator('#artifact-comment').evaluate(e=>e.setSelectionRange(5,9));
  const scroll=await page.evaluate(()=>scrollY);
  artifact.title='Updated by another editor';artifact.version++;
  artifact.body_html+='<p>New content from another editor.</p>';
  await page.waitForFunction(()=>document.querySelector('#artifact-title')===null&&document.querySelector('.artifact-document-heading h1')?.textContent==='Updated by another editor');
  check(await page.locator('#artifact-reading').innerText().then(s=>s.includes('New content from another editor.')),'Document changes render without reloading');
  check(await page.locator('#artifact-comment').evaluate(e=>e===document.activeElement&&e.value==='Keep this draft while the document changes'&&e.selectionStart===5&&e.selectionEnd===9),'Refresh preserves the active draft, focus, and caret');
  check(await page.evaluate(scroll=>Math.abs(scrollY-scroll)<2,scroll),'Refresh preserves reading position');
  check(await page.evaluate(()=>CSS.highlights.get('artifact-comment-selection')?.size===1),'Refresh restores the selected passage highlight');
  comments.push({id:comments.length+1,author:'boss',body:'Another reviewer commented',body_html:'<p>Another reviewer commented</p>',created_at:new Date().toISOString()});
  await page.waitForFunction(()=>document.querySelector('#artifact-threads')?.textContent.includes('Another reviewer commented'));
  check(await page.locator('#artifact-comment').evaluate(e=>e===document.activeElement&&e.selectionStart===5),'New comments render without interrupting typing');
  await page.locator('#artifact-comment').evaluate(e=>e.dataset.unchanged='true');
  await page.waitForResponse(response=>response.url().endsWith('/api/action')&&response.request().postDataJSON()?.operation?.operation?.command==='view');
  await page.evaluate(()=>new Promise(requestAnimationFrame));
  check(await page.locator('#artifact-comment').getAttribute('data-unchanged')==='true','Unchanged documents do not re-render');
  check(errors.length===0,'No browser errors');
  await page.unroute('**/api/action',route);
  return checks;
}
