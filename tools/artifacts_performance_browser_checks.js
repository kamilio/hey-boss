// Measure frame gaps while opening and editing isolated stress documents.
async page => {
  const origin=await page.evaluate(()=>location.origin),results=[];
  const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
  const project=boot.projects.find(p=>p.name==='Artifact Performance').id;
  const response=await page.request.post(origin+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation:{command:'list',archived:false}}}});
  const rows=(await response.json()).artifacts.filter(a=>/^Reading stress \d+$/.test(a.title)).sort((a,b)=>Number(a.title.split(' ').pop())-Number(b.title.split(' ').pop()));
  await page.setViewportSize({width:390,height:844});
  await page.goto(origin+'/artifacts#project='+encodeURIComponent(project));await page.reload();
  await page.locator('.artifact-row').first().waitFor();
  for(const row of rows) {
    await page.evaluate(()=>{
      window.artifactProbe={frames:[],started:performance.now(),stop:false};
      let previous=performance.now();const tick=now=>{artifactProbe.frames.push(now-previous);previous=now;if(!artifactProbe.stop)requestAnimationFrame(tick);};requestAnimationFrame(tick);
    });
    await page.goto(origin+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+row.id);
    await page.locator('#artifact-reading').waitFor();
    await page.waitForFunction(()=>!document.querySelector('#artifact-reading').hasAttribute('aria-busy'));
    await page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,1000)));
    const reading=await page.evaluate(()=>{artifactProbe.stop=true;return {maxFrameGap:Math.max(...artifactProbe.frames),elapsed:performance.now()-artifactProbe.started,blocks:document.querySelector('#artifact-reading').children.length};});
    await page.locator('#artifact-edit').click();
    if(Number(row.title.split(' ').pop())>32000)await page.locator('.cm-content').waitFor();
    const field=page.getByRole('textbox',{name:'Markdown',exact:true});
    const virtual=await field.evaluate(el=>el.isContentEditable);
    const typing=virtual?null:await field.evaluate(async field=>{
      const events=[],writes=[];const original=Storage.prototype.setItem;
      Storage.prototype.setItem=function(key,value){const started=performance.now();const result=original.call(this,key,value);writes.push(performance.now()-started);return result;};
      try{for(let i=0;i<20;i++){const started=performance.now();field.value+='a';field.dispatchEvent(new Event('input'));events.push(performance.now()-started);await new Promise(resolve=>setTimeout(resolve,20));}await new Promise(resolve=>setTimeout(resolve,400));}
      finally{Storage.prototype.setItem=original;}
      return {maxInputTime:Math.max(...events),draftWrites:writes.length,maxDraftWriteTime:Math.max(0,...writes)};
    });
    await field.focus();
    await field.press('ControlOrMeta+End');
    await field.evaluate(field=>{
      window.keyboardProbe={events:[],frames:[],stop:false};
      field.addEventListener('beforeinput',()=>{keyboardProbe.inputStart=performance.now();});
      field.addEventListener('input',()=>{keyboardProbe.events.push(performance.now()-keyboardProbe.inputStart);});
      let previous=performance.now();const tick=now=>{keyboardProbe.frames.push(now-previous);previous=now;if(!keyboardProbe.stop)requestAnimationFrame(tick);};requestAnimationFrame(tick);
    });
    await field.pressSequentially(' A smoother writing experience.',{delay:20});
    await page.waitForTimeout(400);
    const keyboard=await page.evaluate(()=>{keyboardProbe.stop=true;return {maxInputTime:Math.max(...keyboardProbe.events),maxFrameGap:Math.max(...keyboardProbe.frames),characters:keyboardProbe.events.length};});
    results.push({title:row.title,reading,typing,keyboard,virtualEditor:virtual});
    await page.locator('#artifact-cancel').click();
  }
  return {engine:page.context().browser().browserType().name(),results};
}
