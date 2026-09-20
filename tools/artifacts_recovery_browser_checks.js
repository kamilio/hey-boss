// Read failures recover in place; imports, downloads and keyboard save remain usable.
async page => {
  const origin=await page.evaluate(()=>location.origin),checks=[];
  await page.setViewportSize({width:390,height:844});
  await page.unroute('**/api/action');
  let fail=true;
  await page.route('**/api/action',async route=>{
    const command=route.request().postDataJSON()?.operation?.operation?.command;
    if(fail&&command==='list'){await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({error:'Connection unavailable'})});return;}
    await route.continue();
  });
  await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio');await page.reload();
  await page.getByText('Connection unavailable',{exact:true}).waitFor();
  fail=false;await page.getByRole('button',{name:'Try again',exact:true}).click();
  await page.locator('.artifact-row').first().waitFor();
  if(await page.locator('#artifact-error').isVisible())throw Error('Recovered error stayed visible');
  checks.push('Library failure retries in place');
  await page.unroute('**/api/action');
  await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio&new=1&issue=1');
  await page.getByRole('button',{name:'Back',exact:true}).click();
  await page.waitForURL('**/#project=named%3AArtifact+Studio&issue=1');
  checks.push('New document returns to referring issue');
  await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio&new=1');
  await page.getByRole('textbox',{name:'Title',exact:true}).fill('Keyboard/import verification '+Date.now());
  await page.locator('#artifact-import').setInputFiles('output/playwright/artifact-redesign/import-notes.md');
  await page.waitForFunction(()=>document.querySelector('#artifact-body').value.startsWith('# Imported notes'));
  await page.locator('#artifact-body').press('ControlOrMeta+s');
  await page.locator('#artifact-reading strong').waitFor();
  await page.locator('.artifact-menu summary').click();
  const downloadPromise=page.waitForEvent('download');
  await page.getByRole('button',{name:'Export Markdown',exact:true}).click();
  const download=await downloadPromise;
  await download.saveAs('output/playwright/artifact-redesign/'+page.context().browser().browserType().name()+'-export.md');
  checks.push('Import, keyboard save and Markdown export');
  await page.locator('.artifact-menu summary').click();
  await page.getByRole('button',{name:'Archive',exact:true}).click();
  await page.getByRole('button',{name:'Edit',exact:true}).waitFor();
  await page.getByRole('link',{name:'All artifacts',exact:true}).click();
  await page.locator('#artifact-archived').check();
  await page.getByText('Archived',{exact:true}).first().waitFor();
  checks.push('Archived filter');
  return {checks,download:download.suggestedFilename()};
}
