// Production paired page, queued authoritative reads/mutations and shared resource links.
async page => {
  const base='http://127.0.0.1:59489',checks=[];
  const code=(await(await page.request.get(base+'/fixture-pairing')).json()).code;
  const pair=await page.request.post(base+'/api/pair',{data:{code}});if(!pair.ok())throw Error('Synthetic pairing failed');
  await page.setViewportSize({width:390,height:844});await page.goto(base+'/artifacts#project=named%3AArtifact+Studio');await page.reload();
  await page.locator('.artifact-row').first().waitFor();
  await page.getByRole('button',{name:'New artifact',exact:true}).click();
  const title='Paired mobile verification '+Date.now();
  await page.getByRole('textbox',{name:'Title',exact:true}).fill(title);
  await page.getByRole('textbox',{name:'Markdown',exact:true}).fill('# A mobile document\n\nShared **selected passage**.');
  await page.getByRole('button',{name:'Save',exact:true}).click();await page.locator('#artifact-reading strong').waitFor();
  checks.push('Paired create delivered through native bridge');
  await page.evaluate(()=>{const text=document.querySelector('#artifact-reading strong').firstChild,range=document.createRange();range.selectNodeContents(text);getSelection().removeAllRanges();getSelection().addRange(range);document.querySelector('#artifact-reading').dispatchEvent(new Event('pointerup'));});
  await page.getByRole('button',{name:'Comment on selection',exact:true}).click();
  await page.getByRole('textbox',{name:'Add a comment',exact:true}).fill('Phone discussion');
  await page.getByRole('button',{name:'Comment',exact:true}).click();await page.getByText('Phone discussion',{exact:true}).waitFor();
  checks.push('Phone quoted comment delivered and retained');
  const hash=await page.evaluate(()=>location.hash);
  await page.reload();await page.locator('#artifact-reading').waitFor();
  await page.getByRole('button',{name:'Comments',exact:false}).click();await page.getByText('Phone discussion',{exact:true}).waitFor();
  checks.push('Authoritative phone reload');
  await page.goto(base+'/project-resource#project=named%3AArtifact+Studio&issue=1');
  await page.locator('#resource-artifacts [data-attach]').click();
  await page.getByRole('searchbox',{name:'Find artifact to attach'}).fill(title);
  await page.getByRole('button',{name:'Attach',exact:true}).click();await page.getByRole('link',{name:title,exact:true}).waitFor();
  await page.getByRole('link',{name:title,exact:true}).click();
  await page.locator('.artifact-backlinks a').waitFor();
  if(!await page.locator('.artifact-backlinks a').getAttribute('href').then(href=>href.startsWith('/project-resource#')))throw Error('Phone backlink uses native route');
  if((await page.evaluate(()=>document.documentElement.scrollWidth))>390)throw Error('Phone page overflow');
  await page.screenshot({path:'output/playwright/artifact-redesign/'+page.context().browser().browserType().name()+'-paired-mobile-reader.png',fullPage:true});
  checks.push('Shared referring-resource attachment and phone backlink');
  return {checks,hash};
}
