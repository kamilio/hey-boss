// Saved, hidden detail drafts must not trap a user on the list page.
async page => {
  const checks=[],check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name)};
  page.on('dialog',dialog=>dialog.accept().catch(()=>{}));
  await page.reload();await page.waitForFunction(()=>typeof model!=='undefined'&&model.csrf&&model.project);
  const origin=await page.evaluate(()=>location.origin),project=await page.evaluate(async()=>(await api({action:'create',title:'Exit navigation fixture',body:'',labels:[]},'Exit navigation '+crypto.randomUUID())).project.id);
  const go=async()=>{await page.goto(origin+'/#project='+encodeURIComponent(project)+'&issue=1');await page.waitForFunction(project=>model.project?.id===project&&model.detail?.issue.number===1,project)};
  const warns=()=>page.evaluate(()=>{const e=new Event('beforeunload',{cancelable:true});dispatchEvent(e);return e.defaultPrevented});
  await go();await page.locator('#comment-body').fill('Comment draft survives leaving the issue');
  check(await warns(),'Visible comment draft receives exit protection');
  await page.locator('[data-back]').click();
  check(!await warns(),'Saved hidden comment does not warn on the list page');
  check(await page.evaluate(project=>storage.get(draftKey('comment',project,1))==='Comment draft survives leaving the issue',project),'Leaving detail saves its comment draft');
  page.removeAllListeners('dialog');let dialogs=0;
  page.on('dialog',dialog=>{dialogs++;dialog.accept().catch(()=>{})});
  await page.reload({waitUntil:'domcontentloaded'});await page.waitForFunction(()=>typeof model!=='undefined'&&model.csrf&&model.signature);
  check(dialogs===0,'Reloading the list requires no native confirmation');
  await go();check(await page.locator('#comment-body').inputValue()==='Comment draft survives leaving the issue','Reload and reopening restore the comment');
  await page.locator('#comment-body').fill('');check(!await warns(),'Empty visible comment does not warn');
  await page.locator('[data-back]').click();await page.locator('#new-issue').click();await page.locator('#editor-subject').fill('Protected new issue draft');
  check(await warns(),'Visible new issue draft retains exit protection');
  await page.keyboard.press('Escape');check(!await warns(),'Cancelling the issue editor releases exit protection');
  return {passed:checks.length,checks};
}
