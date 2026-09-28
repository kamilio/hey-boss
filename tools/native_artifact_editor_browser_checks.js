async page => {
  const checks=[];
  const assert=(value,label)=>{if(!value)throw Error(label);checks.push(label);};
  await page.locator('#artifact-reading').waitFor();
  await page.getByRole('button',{name:'Edit',exact:true}).click();
  await page.getByText('Opened in native editor · auto-saving',{exact:true}).waitFor();
  assert(await page.locator('#artifact-editor').count()===0,'Desktop Edit opens native without a second editing session');
  await page.keyboard.press('Meta+e');
  await page.getByText('Opened in native editor · auto-saving',{exact:true}).waitFor();
  await page.locator('.artifact-menu summary').click();
  await page.getByRole('button',{name:'Edit in browser',exact:true}).click();
  await page.getByRole('textbox',{name:'Markdown',exact:true}).fill('Unsent browser draft');
  await page.evaluate(()=>window.dispatchEvent(new Event('focus')));
  await page.waitForTimeout(1700);
  assert(await page.getByRole('textbox',{name:'Markdown',exact:true}).inputValue()==='Unsent browser draft','Native synchronization never overwrites browser typing');
  assert(await page.locator('#artifact-editor-native').count()===0,'No unsaved draft is silently handed to a second editor');
  return checks;
}
