// Scrolling checks for an isolated Artifact Studio fixture.
async page => {
  const origin=await page.evaluate(()=>location.origin),checks=[];
  const check=(ok,name)=>{if(!ok)throw Error(name);checks.push(name);};
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.setViewportSize({width:390,height:844});
  await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio');
  await page.reload();
  await page.locator('.artifact-row').filter({hasText:'Workspace design notes'}).click();
  await page.locator('#artifact-reading').waitFor();
  await page.locator('#artifact-comments-toggle').click();
  check(await page.locator('.artifact-comments-heading').evaluate(el=>el.getBoundingClientRect().top>=0&&el.getBoundingClientRect().bottom<=innerHeight),'Opening comments on a phone brings the conversation into view');
  await page.locator('#artifact-comments-close').click();
  check(await page.locator('.artifact-document-heading').evaluate(el=>el.getBoundingClientRect().top>=0&&el.getBoundingClientRect().bottom<=innerHeight),'Closing comments returns to document controls');
  await page.locator('#artifact-edit').click();
  await page.locator('#artifact-body').fill(Array.from({length:40},(_,i)=>'Paragraph '+i+' with several lines of project notes.').join('\n\n'));
  check(await page.locator('#artifact-body').evaluate(el=>el.scrollHeight<=el.clientHeight+2),'Writing uses the page scroll rather than a nested scrollbar');
  await page.evaluate(()=>scrollTo(0,document.documentElement.scrollHeight));
  check(await page.getByRole('button',{name:'Save',exact:true}).evaluate(el=>el.getBoundingClientRect().top>=0&&el.getBoundingClientRect().bottom<=innerHeight),'Save stays reachable while reading a long draft');
  return {checks};
}
