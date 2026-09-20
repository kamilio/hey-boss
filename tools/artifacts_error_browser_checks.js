// Failed reads stop looking like loading and always leave a way back.
async page => {
  const origin=await page.evaluate(()=>location.origin);
  await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio&artifact=missing-document');await page.reload();
  await page.locator('#artifact-error').waitFor();
  if(await page.locator('.artifact-loading').count())throw Error('Failed document still shows a loading skeleton');
  await page.getByRole('link',{name:'All artifacts',exact:true}).click();
  await page.locator('.artifact-row').first().waitFor();
  return {loadingClearedOnFailure:true,returnToLibrary:true};
}
