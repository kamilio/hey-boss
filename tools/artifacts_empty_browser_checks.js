// The Empty Artifact Studio project must have no active documents.
async page => {
  const origin=await page.evaluate(()=>location.origin);
  await page.setViewportSize({width:390,height:844});
  await page.goto(origin+'/artifacts#project='+encodeURIComponent('named:Empty Artifact Studio'));await page.reload();
  await page.getByText('Your documents start here',{exact:true}).waitFor();
  if(await page.getByRole('button',{name:'New artifact',exact:true}).count()!==1)throw Error('Empty library repeats its primary action');
  await page.getByRole('button',{name:'New artifact',exact:true}).click();
  await page.getByRole('textbox',{name:'Title',exact:true}).waitFor();
  await page.getByRole('button',{name:'Back',exact:true}).click();
  await page.getByText('Your documents start here',{exact:true}).waitFor();
  if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1))throw Error('Empty state overflows the phone viewport');
  return {singlePrimaryAction:true,newEditorReachable:true,backPreservesEmptyState:true};
}
