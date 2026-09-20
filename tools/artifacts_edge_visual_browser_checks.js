// Recovery and keyboard/responsive states outside the standard visual matrix.
async page => {
 await page.unroute('**/api/action');
 const origin='http://127.0.0.1:59488',engine=page.context().browser().browserType().name(),checks=[];
 const screenshot=async name=>{if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1))throw Error('Overflow '+name);await page.screenshot({path:'output/playwright/artifact-redesign/'+engine+'-edge-'+name+'.png'});checks.push(name);};
 for(const theme of ['light','dark']){
  await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});await page.setViewportSize({width:390,height:844});
  await page.goto(origin+'/artifacts#project=named%3AEmpty+Artifact+Studio');await page.reload();await page.locator('.artifact-empty').waitFor();await screenshot(theme+'-empty');
  await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio');await page.locator('.artifact-row').first().waitFor();
  await page.locator('#artifact-search').fill('unmatched-documents-xyz');await page.getByText('No matching artifacts',{exact:true}).waitFor();await screenshot(theme+'-search-empty');await page.locator('#artifact-empty-action').click();
  await page.route('**/api/action',async route=>{await page.waitForTimeout(1500);await route.continue();});
  await page.reload();await page.locator('.artifact-loading').waitFor();await screenshot(theme+'-library-loading');await page.locator('.artifact-row').first().waitFor();await page.unroute('**/api/action');
  await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio&artifact=missing-document');await page.locator('#artifact-error').waitFor();await screenshot(theme+'-document-error');
  await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio&artifact=a-ee0c2b7d6363d84512afd029a70c6bcf');await page.locator('#artifact-reading').waitFor();
  await page.locator('.artifact-menu summary').focus();await page.keyboard.press('Enter');await page.locator('#artifact-export').waitFor();await screenshot(theme+'-keyboard-menu');await page.keyboard.press('Escape');
  if(await page.locator('.artifact-menu').getAttribute('open')!==null)throw Error('Escape did not close menu');
  await page.locator('#artifact-edit').click();await page.locator('#artifact-body').fill('# Rotating a draft\n\nKeep this text exactly.');
  for(const [label,width,height] of [['landscape',844,390],['zoom-equivalent',720,450],['portrait',390,844]]){
   await page.setViewportSize({width,height});await screenshot(theme+'-'+label);if(await page.locator('#artifact-body').inputValue()!=='# Rotating a draft\n\nKeep this text exactly.')throw Error('Resize changed draft');
  }
 }
 return {checks};
}
