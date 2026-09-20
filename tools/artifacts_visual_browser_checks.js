// Visual matrix for the synthetic Artifact Studio fixture, using playwright-cli.
async page => {
  const origin=await page.evaluate(()=>location.origin);
  const engine=page.context().browser().browserType().name();
  const errors=[],checks=[],screenshots=[];
  page.on('pageerror',e=>errors.push(e.message));
  const check=async name=>{
    const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1);
    if(overflow)throw Error('Page overflow: '+name);
    checks.push(name);
    const path='output/playwright/artifact-redesign/'+engine+'-'+name+'.png';
    await page.screenshot({path,fullPage:true,caret:'initial'});screenshots.push(path);
  };
  for(const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    for(const [size,width,height] of [['desktop',1440,1000],['tablet',820,1180],['phone',390,844],['small-phone',320,740]]) {
      await page.setViewportSize({width,height});
      await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio');await page.reload();
      await page.locator('.artifact-row').first().waitFor();
      await page.evaluate(()=>{document.activeElement.blur();scrollTo(0,0);});
      await check(theme+'-'+size+'-library');
      await page.locator('.artifact-row').filter({hasText:'Workspace design notes'}).click();
      await page.locator('#artifact-reading').waitFor();
      await page.evaluate(()=>scrollTo(0,0));
      await check(theme+'-'+size+'-reader');
      if(!await page.locator('#artifact-comments').isVisible())await page.locator('#artifact-comments-toggle').click();
      await check(theme+'-'+size+'-comments');
      await page.locator('#artifact-edit').click();
      // Avoid publishing or replacing fixture drafts during visual review.
      await page.locator('#artifact-body').fill('# A calmer workspace\n\nKeep the important things close. Give everything else room to breathe.\n\n## What we’re building\n\nA simple home for **project documents**, decisions, and conversations.\n\n- [x] A clear reading surface\n- [ ] Review the mobile experience\n\n> Good tools make the next step obvious.');
      await page.evaluate(()=>{document.activeElement.blur();scrollTo(0,0);});
      await check(theme+'-'+size+'-editor');
      await page.locator('#artifact-preview').click();
      await page.locator('#artifact-edit-preview strong').waitFor();
      await check(theme+'-'+size+'-preview');
    }
  }
  if(errors.length)throw Error(errors.join('\n'));
  return {checks,screenshots,scriptErrors:errors};
}
