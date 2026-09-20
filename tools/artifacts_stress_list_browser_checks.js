// A giant semantic list should appear without one full-document main-thread parse.
async page => {
  const origin=await page.evaluate(()=>location.origin),project='named:Artifact Performance';
  const boot=await(await page.request.get(origin+'/api/bootstrap')).json();
  const body='# Maximum checklist\n\n'+Array.from({length:28000},(_,i)=>'- [x] Checklist item '+i+'.').join('\n');
  const response=await page.request.post(origin+'/api/action',{headers:{'X-Hey-Boss-CSRF':boot.csrf},data:{project,operation:{action:'artifact',operation:{command:'create',title:'Maximum checklist stress',body}},request_id:await page.evaluate(()=>crypto.randomUUID())}});
  if(!response.ok())throw Error(await response.text());const saved=(await response.json()).artifact;
  await page.goto(origin+'/artifacts#project='+encodeURIComponent(project));await page.reload();await page.locator('.artifact-row').first().waitFor();
  await page.evaluate(()=>{window.listProbe={frames:[],stop:false};let previous=performance.now();const tick=now=>{listProbe.frames.push(now-previous);previous=now;if(!listProbe.stop)requestAnimationFrame(tick);};requestAnimationFrame(tick);});
  await page.goto(origin+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+saved.id);
  await page.locator('#artifact-reading').waitFor();await page.waitForFunction(()=>!document.querySelector('#artifact-reading').hasAttribute('aria-busy'));
  // Finish the frame probe before sending megabytes of canonical HTML through
  // the automation bridge; compiling that comparison is not reader work.
  const timing=await page.evaluate(()=>{listProbe.stop=true;return {maxFrameGap:Math.max(...listProbe.frames)};});
  const semantics=await page.locator('#artifact-reading').evaluate((reader,html)=>{const canonical=document.createElement('template');canonical.innerHTML=html;return {items:reader.querySelectorAll('ul > li').length,canonicalHTML:reader.innerHTML===canonical.innerHTML};},saved.body_html);
  const result={...timing,...semantics};
  if(result.items!==28000||!result.canonicalHTML)throw Error('Checklist lost its complete semantic HTML');
  if(result.maxFrameGap>150)throw Error('Checklist freezes the reader: '+JSON.stringify(result));
  return result;
}
