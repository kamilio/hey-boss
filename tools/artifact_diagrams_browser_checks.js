// Run via playwright-cli run-code on serve_artifact_diagrams_fixture.mjs.
async page => {
  const checks=[],errors=[];
  const assert=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
    window.diagramPolicyViolations=[];
    document.addEventListener('securitypolicyviolation',e=>window.diagramPolicyViolations.push(e.violatedDirective));
  });
  const origin=await page.evaluate(()=>location.origin);
  const mobile=origin.endsWith(':59550');
  await page.reload();
  if(mobile){
    const pair=await(await page.request.get(origin+'/fixture-pairing')).json();
    const response=await page.request.post(origin+'/api/pair',{data:{code:pair.code,device:'Diagram QA'}});
    assert(response.ok(),'Mobile device pairs with isolated store');
  }
  await page.goto(origin+'/artifacts#project=named%3AFlowchart+QA');
  await page.reload();
  await page.locator('.artifact-row').first().click();
  const figures=page.locator('.artifact-diagram');
  await figures.first().locator('svg').waitFor();
  assert(await figures.count()===4,'Only Mermaid fences become diagram figures');
  assert(await page.locator('pre > code.language-js').count()===1,'Ordinary fenced code remains code');
  assert(await figures.first().locator('svg text').count()>10,'Recovery flow renders all service labels');
  assert(await figures.first().locator('svg .node text').first().evaluate(n=>{const s=getComputedStyle(n);return s.stroke==='none'&&parseFloat(s.fontSize)>=14;}),'Diagram labels retain readable typography without icon strokes');
  assert(await figures.first().locator('svg .edgePath, svg .edgePaths path').count()>10,'Flowchart arrows render');
  await figures.nth(2).scrollIntoViewIfNeeded();
  await page.waitForFunction(()=>document.querySelectorAll('.artifact-diagram')[2]?.dataset.state==='error');
  assert(await figures.nth(2).locator('details').getAttribute('open')!==null,'Malformed chart automatically reveals exact source');
  await figures.nth(3).scrollIntoViewIfNeeded();
  await page.waitForFunction(()=>['ready','error'].includes(document.querySelectorAll('.artifact-diagram')[3]?.dataset.state));
  assert(await page.evaluate(()=>!window.diagramPwned),'Untrusted labels and click directives cannot execute');
  assert(await figures.nth(3).locator('svg a[href^="javascript"],svg script,svg foreignObject').count()===0,'SVG has no scripts, unsafe links or HTML labels');
  await figures.first().locator('[data-expand]').click();
  const dialog=page.getByRole('dialog',{name:'Expanded diagram'});
  await dialog.waitFor();
  assert(await page.locator('[id]').evaluateAll(nodes=>{const ids=nodes.map(n=>n.id);return new Set(ids).size===ids.length;}),'Expanded SVG keeps unique IDs');
  const initialZoom=await dialog.locator('output').innerText();
  await dialog.getByRole('button',{name:'Zoom in',exact:true}).click();
  assert(parseInt(await dialog.locator('output').innerText())>parseInt(initialZoom),'Zoom changes diagram scale');
  await dialog.getByRole('button',{name:'Actual size',exact:true}).click();
  assert(await dialog.locator('output').innerText()==='100%','Actual size makes wide diagram labels readable at any screen width');
  await dialog.getByRole('button',{name:'Fit',exact:true}).click();
  assert(await dialog.locator('output').innerText()===initialZoom,'Fit restores overview scale');
  await page.keyboard.press('Escape');
  assert(await page.locator('.artifact-diagram-dialog').count()===0,'Escape removes viewer');
  assert(await figures.first().locator('[data-expand]').evaluate(n=>n===document.activeElement),'Closing viewer restores keyboard focus');
  assert(await figures.first().locator('svg').count()===1,'Closing viewer restores inline diagram');

  for(const theme of ['light','dark']){
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for(const [size,width,height] of [['desktop',1440,1000],['tablet',820,1180],['phone',390,844],['small-phone',320,740]]){
      await page.setViewportSize({width,height});
      await figures.first().scrollIntoViewIfNeeded();
      await page.waitForFunction(()=>document.querySelector('.artifact-diagram')?.dataset.state==='ready');
      await page.screenshot({path:`output/playwright/issue49/${mobile?'mobile':'native'}-${theme}-${size}.png`,fullPage:true});
      assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),`${theme} ${size} reading stays within viewport`);
      await figures.first().locator('[data-expand]').click();
      await dialog.waitFor();
      assert(await dialog.evaluate(n=>{const r=n.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth+1&&r.top>=0&&r.bottom<=innerHeight+1;}),`${theme} ${size} expanded viewer fits screen`);
      await page.screenshot({path:`output/playwright/issue49/${mobile?'mobile':'native'}-${theme}-${size}-expanded.png`});
      await page.keyboard.press('Escape');
    }
  }
  await page.locator('#artifact-edit').click();
  await page.locator('#artifact-preview').click();
  await page.locator('#artifact-edit-preview .artifact-diagram svg').first().waitFor();
  assert(true,'Editor preview renders diagrams without modifying source');
  await page.locator('#artifact-preview').click();
  assert((await page.locator('#artifact-body').inputValue()).includes('```mermaid\nflowchart LR'),'Write mode preserves fenced Mermaid source');
  await page.locator('#artifact-cancel').click();
  await page.locator('#artifact-reading').waitFor();
  await page.evaluate(()=>{
    const heading=[...document.querySelectorAll('#artifact-reading h2')].find(n=>n.textContent==='Ordinary code');
    const range=document.createRange();range.selectNodeContents(heading);
    const selection=getSelection();selection.removeAllRanges();selection.addRange(range);
    document.querySelector('#artifact-reading').dispatchEvent(new Event('pointerup'));
  });
  await page.getByRole('button',{name:'Comment on selection',exact:true}).click();
  await page.getByRole('textbox',{name:'Add a comment',exact:true}).fill('Diagram-safe anchor');
  await page.getByRole('button',{name:'Comment',exact:true}).click();
  await page.getByText('Diagram-safe anchor',{exact:true}).last().waitFor();
  assert(await page.getByText('Outdated selection · discussion preserved',{exact:true}).count()===0,'Diagram controls do not change prose comment anchors');
  assert(await page.evaluate(()=>window.diagramPolicyViolations.length===0),'Rendering satisfies production Content Security Policy');
  assert(errors.length===0,'No browser script errors');
  return {checks,scriptErrors:errors,mobile};
}
