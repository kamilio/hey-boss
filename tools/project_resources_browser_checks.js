// Run against an isolated server with Existing Resources and Empty Resources disabled.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  const origin = 'http://127.0.0.1:59644';
  await page.setViewportSize({width:390,height:844});
  await page.goto(origin + '/artifacts#project=named%3AEmpty+Resources');
  await page.getByText('Artifacts are turned off', {exact:true}).waitFor();
  check(await page.getByRole('button', {name:'New artifact',exact:true}).count() === 0, 'Empty library does not invite disabled creation');
  check(await page.getByText('This project is not using artifacts. Use regular tasks. Existing documents remain available.').isVisible(), 'Empty state explains regular tasks');
  await page.goto(origin + '/artifacts#project=named%3AEmpty+Resources&new=1');
  await page.getByText('Project "Empty Resources" is not using artifacts. Use regular tasks.',{exact:true}).waitFor();
  check(await page.getByRole('textbox', {name:'Title',exact:true}).count() === 0, 'Direct creation URL is blocked');
  await page.goto(origin + '/artifacts#project=named%3AExisting+Resources');
  await page.locator('.artifact-row-link').filter({hasText:'Existing document'}).waitFor();
  check(await page.getByRole('button', {name:'New artifact',exact:true}).count() === 0, 'Existing library hides disabled creation');
  await page.locator('.artifact-row-link').filter({hasText:'Existing document'}).click();
  await page.locator('#artifact-reading').getByText('Preserved content',{exact:true}).waitFor();
  check(await page.getByRole('button', {name:'Edit',exact:true}).isVisible(), 'Existing content stays editable');
  check(await page.getByRole('button', {name:'New artifact',exact:true}).count() === 0, 'Reading does not invite disabled creation');
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'No mobile overflow');
  check(errors.length === 0, 'No browser exceptions: ' + errors.join(', '));
  return {checks};
}
