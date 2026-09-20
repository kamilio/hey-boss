// A maximum-size draft must support real typing, undo and a complete reload.
async page => {
  const origin=await page.evaluate(()=>location.origin),project='named:Artifact Performance';
  const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
  const response=await page.request.post(origin+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation:{command:'list',archived:false}}}});
  const row=(await response.json()).artifacts.find(a=>a.title==='Reading stress 1000000');
  await page.setViewportSize({width:390,height:844});
  await page.goto(origin+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+row.id);await page.reload();
  await page.evaluate(()=>{window.editorCSP=[];document.addEventListener('securitypolicyviolation',e=>editorCSP.push(e.violatedDirective));});
  await page.locator('#artifact-edit').click();
  const field=page.getByRole('textbox',{name:'Markdown',exact:true});
  if(!await field.evaluate(el=>el.isContentEditable))throw Error('Large draft still uses the slow full-document textarea');
  if((await page.evaluate(()=>editorCSP)).length)throw Error('Writing surface violates CSP');
  await field.focus();await field.press('ControlOrMeta+ArrowDown');
  await page.evaluate(()=>{
    window.editorProbe={frames:[],stop:false};let previous=performance.now();
    const tick=now=>{editorProbe.frames.push(now-previous);previous=now;if(!editorProbe.stop)requestAnimationFrame(tick);};requestAnimationFrame(tick);
  });
  await field.pressSequentially(' End-of-document typing check.',{delay:20});await page.waitForTimeout(500);
  const maxFrameGap=await page.evaluate(()=>{editorProbe.stop=true;return Math.max(...editorProbe.frames);});
  if(!await page.evaluate(id=>JSON.parse(localStorage.getItem('hey-boss-artifact-draft:local:named:Artifact Performance:'+id)).body.endsWith('End-of-document typing check.'),row.id))throw Error('Keyboard did not reach the complete document end');
  await field.press('ControlOrMeta+z');await field.press('ControlOrMeta+Shift+z');
  await page.waitForTimeout(400);
  await page.reload();await page.locator('#artifact-edit').click();await page.getByRole('textbox',{name:'Title',exact:true}).waitFor();
  await page.getByRole('textbox',{name:'Markdown',exact:true}).press('ControlOrMeta+ArrowDown');
  if(!await page.locator('.cm-content').innerText().then(text=>text.includes('End-of-document typing check.')))throw Error('Typing/redo lost the final draft text');
  await page.getByRole('button',{name:'Preview',exact:true}).click();
  await page.waitForFunction(()=>{const p=document.querySelector('#artifact-edit-preview');return !p.hidden&&!p.hasAttribute('aria-busy');});
  if(!await page.locator('#artifact-edit-preview').textContent().then(text=>text.includes('End-of-document typing check.')))throw Error('Preview lost large draft content');
  await page.getByRole('button',{name:'Write',exact:true}).click();
  await page.screenshot({path:'output/playwright/artifact-redesign/'+page.context().browser().browserType().name()+'-large-editor.png'});
  return {maxFrameGap,largeDraftReload:true,undoRedo:true,completePreview:true};
}
