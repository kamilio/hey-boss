// Tight real keyboard cycles test native dialog focus restoration under polling.
async page => {
 await page.reload();await page.waitForFunction(()=>model.csrf&&model.project);
 const origin=await page.evaluate(()=>location.origin),project=await page.evaluate(async()=>{const r=await api({action:'create',title:'Keyboard fixture',body:'',labels:[]},'Keyboard dialogs '+crypto.randomUUID());return r.project.id});
 await page.goto(origin+'/#project='+encodeURIComponent(project));await page.waitForFunction(project=>model.project?.id===project&&model.signature,project);
 const errors=[];page.on('pageerror',e=>errors.push(e.message));let checks=0;
 for(let i=0;i<300;i++){
  await page.locator('#new-issue').focus();await page.keyboard.press('n');if(!await page.locator('#editor-dialog').evaluate(el=>el.open))throw Error('N failed: '+i);checks++;
  await page.locator('#editor-subject').fill('Keyboard draft '+i);await page.keyboard.press('Escape');await page.keyboard.press('n');
  if(!await page.locator('#editor-dialog').evaluate(el=>el.open))throw Error('Second N failed: '+i);checks++;
  if(await page.locator('#editor-subject').inputValue()!=='Keyboard draft '+i)throw Error('Draft failed: '+i);checks++;
  await page.keyboard.press('Escape');await page.keyboard.press('/');
  const state=await page.evaluate(()=>({active:document.activeElement.id,route:{...model.route},dialogs:[...document.querySelectorAll('dialog[open]')].map(d=>d.id)}));
  if(state.active!=='issue-search')throw Error('Slash failed in cycle '+i+': '+JSON.stringify(state));checks++;
 }
 if(errors.length)throw Error(errors.join(';'));return {passed:checks,cycles:300,errors};
}
