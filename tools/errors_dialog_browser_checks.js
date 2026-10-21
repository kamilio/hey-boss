// Run against a disposable real server; this creates synthetic tasks only.
async page => {
  const assert = (value, message) => { if (!value) throw Error(message); };
  await page.waitForFunction(() => typeof model !== 'undefined' && model.csrf);
  const project = 'named:Error dialog QA';
  await page.evaluate(async project => {await api({action:'project_init',settings:{prs_enabled:false,worktree_enabled:false,if_version:0}},project);},project);
  await page.goto(new URL('/errors#'+new URLSearchParams({project}), page.url()).href);
  await page.getByRole('button', {name:'Enable collection',exact:true}).click();
  await page.getByRole('button', {name:'Collection on',exact:true}).waitFor();
  await page.evaluate(async project => {
    const boot = await (await fetch('/api/bootstrap')).json();
    await HeyBossUI.post('/api/errors',{action:'ingest',project,events:Array.from({length:10},(_,i)=>({source:'synthetic',event_id:String(i),timestamp:Date.now()-i*1000,message:'Synthetic upload failure',stack:'at upload (storage.rs:42)\n'.repeat(30),count:1}))},boot);
  },project);
  await page.getByRole('button',{name:'Refresh errors',exact:true}).click();
  const open = () => page.getByRole('button',{name:'Synthetic upload failure',exact:true}).click();
  const dialog = page.locator('#error-dialog');
  await page.locator('#error-rows').getByRole('button',{name:'Create task',exact:true}).click();
  await page.locator('#error-task-prompt').waitFor();
  const size = await dialog.boundingBox(), viewport=page.viewportSize();
  assert(size.width > viewport.width*.9 && size.height > viewport.height*.9, 'Evidence should be nearly fullscreen');
  assert((await page.locator('#error-task-prompt').inputValue()).startsWith('Fix this error: `hey-boss errors'), 'Default prompt');
  await page.locator('#error-title').click();
  assert(await dialog.evaluate(d=>d.open),'Inside click closed dialog');
  await page.mouse.move(size.x+50,size.y+60);await page.mouse.down();await page.mouse.move(5,5);await page.mouse.up();
  assert(await dialog.evaluate(d=>d.open),'Dragging outside closed dialog');
  await page.mouse.click(5,5);await page.waitForFunction(()=>!document.querySelector('#error-dialog').open);
  await open();await page.locator('#error-task-prompt').fill('Edited error task');
  const failCreation = route => route.request().postDataJSON().action === 'promote'
    ? route.fulfill({status:503,json:{ok:false,error:{message:'Synthetic creation failure'}}})
    : route.continue();
  await page.route('**/api/errors',failCreation);
  await dialog.getByRole('button',{name:'Create task',exact:true}).click();
  await page.getByText('Synthetic creation failure',{exact:true}).waitFor();
  assert(await dialog.evaluate(d=>d.open),'Failed creation closed the dialog');
  assert(await page.locator('#error-task-prompt').inputValue()==='Edited error task','Failed creation lost the draft');
  assert(await page.locator('#error-task-prompt').isEnabled(),'Failed creation left the draft disabled');
  await page.unroute('**/api/errors',failCreation);
  await dialog.getByRole('button',{name:'Create task',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('#error-dialog').open,null,{timeout:5000});
  const taskLink = page.locator('#error-rows').getByRole('link',{name:/^Task #\d+$/});
  await taskLink.waitFor();
  assert(await taskLink.evaluate(el=>document.activeElement===el),'Successful creation should focus its task link');
  assert(await taskLink.getAttribute('href')===await page.locator('#error-rows').getByRole('link',{name:'Open task',exact:true}).getAttribute('href'),'Task links should open the same task');
  await taskLink.click();
  await page.waitForFunction(()=>typeof model!=='undefined'&&model.detail?.issue?.body==='Edited error task');
  await page.keyboard.press('n');await page.locator('#editor-subject').fill('Backdrop draft');
  await page.mouse.click(5,5);await page.waitForFunction(()=>!document.querySelector('#editor-dialog').open);
  await page.keyboard.press('n');
  assert(await page.locator('#editor-subject').inputValue()==='Backdrop draft','Shared dismissal lost the issue draft');
  await page.keyboard.press('Escape');
  await page.locator('#project-settings-trigger').click();
  await page.locator('#settings-tab-instructions').click();
  await page.getByText('Task context sections',{exact:true}).click();
  await page.locator('#project-prompt-error_task').fill('Custom {{command}}');
  await page.locator('#project-settings-form button[type=submit]').click();
  await page.waitForFunction(()=>!document.querySelector('#project-settings-dialog').open);
  assert(await page.evaluate(async()=> (await api({action:'project_settings'})).prompt_overrides.error_task==='Custom {{command}}'),'Project prompt was not saved');
  // Shared pattern: native cancel guards and dynamically inserted dialogs.
  await page.evaluate(()=>{
    const dialog=document.createElement('dialog');dialog.id='dismiss-fixture';dialog.textContent='Dismiss fixture';
    dialog.addEventListener('cancel',e=>{window.cancelCount=(window.cancelCount||0)+1;if(dialog.dataset.block)e.preventDefault();});
    document.body.append(dialog);dialog.showModal();dialog.dataset.block='true';
  });
  await page.mouse.click(5,5);
  assert(await page.locator('#dismiss-fixture').evaluate(d=>d.open),'Cancellation guard ignored');
  await page.evaluate(()=>delete document.querySelector('#dismiss-fixture').dataset.block);
  await page.mouse.click(5,5);await page.waitForFunction(()=>!document.querySelector('#dismiss-fixture').open);
  assert(await page.evaluate(()=>window.cancelCount===2),'Cancel handler did not run exactly once per click');
  await page.evaluate(()=>{const d=document.querySelector('#dismiss-fixture');d.setAttribute('closedby','none');d.showModal();});
  await page.mouse.click(5,5);
  assert(await page.locator('#dismiss-fixture').evaluate(d=>d.open),'Blocking dialog dismissed');
  await page.evaluate(()=>document.querySelector('#dismiss-fixture').remove());
  await page.goto(new URL('/errors#'+new URLSearchParams({project}), page.url()).href);
  await page.setViewportSize({width:390,height:844});await open();await page.getByRole('link',{name:/Open task #/}).waitFor();
  const mobile=await dialog.boundingBox();
  assert(mobile.x>=0&&mobile.x+mobile.width<=390&&mobile.height>780,'Mobile dialog bounds');
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Mobile horizontal overflow');
  await page.keyboard.press('Escape');await page.waitForFunction(()=>!document.querySelector('#error-dialog').open);
  await taskLink.click();
  await page.waitForFunction(()=>typeof model!=='undefined'&&model.detail?.issue?.body==='Edited error task');
  return {desktop:true,mobile:true,prompt:true,projectPromptSaved:true,taskCreated:true,creationClosesDialog:true,failurePreservesDraft:true,indexTaskLink:true,indexAction:true,backdrop:true,draftPreserved:true,dragProtected:true,cancelGuard:true,blocking:true};
}
