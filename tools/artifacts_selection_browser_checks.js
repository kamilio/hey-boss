// Selecting/copying text must keep the reading layout stable until Comment is chosen.
async page => {
  const origin=await page.evaluate(()=>location.origin),checks=[];
  for(const [size,width,height] of [['desktop',1440,1000],['phone',390,844]]) {
    await page.setViewportSize({width,height});
    await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio&artifact=a-ee0c2b7d6363d84512afd029a70c6bcf');await page.reload();
    await page.locator('#artifact-reading').waitFor();
    if(await page.locator('#artifact-comments').isVisible())await page.locator('#artifact-comments-close').click();
    const before=await page.locator('#artifact-reading').boundingBox();
    await page.evaluate(()=>{
      const text=document.querySelector('#artifact-reading h1').firstChild,range=document.createRange();range.selectNodeContents(text);getSelection().removeAllRanges();getSelection().addRange(range);document.querySelector('#artifact-reading').dispatchEvent(new Event('pointerup'));
    });
    if(await page.locator('#artifact-comments').isVisible())throw Error(size+': copying text opened comments');
    const after=await page.locator('#artifact-reading').boundingBox();
    if(before.width!==after.width||before.x!==after.x)throw Error(size+': selection moved the document');
    await page.getByRole('button',{name:'Comment on selection',exact:true}).click();
    await page.locator('#artifact-quote').waitFor();
    if(await page.locator('#artifact-quote').innerText()!=='A calmer workspace')throw Error('Selected quote lost');
    await page.waitForFunction(()=>document.activeElement.id==='artifact-comment');
    if(size==='phone')await page.waitForFunction(()=>{const r=document.querySelector('#artifact-comment').getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;});
    await page.screenshot({path:'output/playwright/artifact-redesign/'+page.context().browser().browserType().name()+'-selection-'+size+'.png'});
    await page.locator('#artifact-clear-quote').click();
    checks.push(size+': stable selection, explicit comment, reachable composer');
  }
  return {checks};
}
