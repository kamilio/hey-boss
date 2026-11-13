// A rename in another session must not interrupt an issue comment.
async page => {
  page.removeAllListeners('dialog');page.on('dialog', d => d.accept().catch(() => {}));
  const origin = await page.evaluate(() => location.origin), checks = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name) };
  await page.goto(origin + '/?detail-profile=' + Date.now());
  await page.waitForFunction(() => model.csrf && model.project);
  const {project, number} = await page.evaluate(async () => {
    const value = await api({ action: 'create', title: 'Profile draft QA', body: '', labels: [] }, model.project.id);
    await api({ action: 'assign_boss', number: value.issue.number, force: false }, value.project.id);
    return {project:value.project.id, number:value.issue.number};
  });
  await page.goto(origin + '/?detail-profile=' + Date.now() + '#project=' + encodeURIComponent(project) + '&issue=' + number);
  await page.waitForFunction(number => model.detail?.issue.number === number, number);
  const input = page.locator('#comment-body');
  await input.fill('Keep typing through a global rename');
  await input.evaluate(el => el.setSelectionRange(5, 11, 'backward'));
  const bootstrap = await (await page.request.get(origin + '/api/bootstrap')).json();
  const action = async operation => {
    const r = await page.request.post(origin + '/api/action', { headers: {'X-Hey-Boss-CSRF': bootstrap.csrf}, data: {project, operation, request_id: null} });
    if (!r.ok()) throw Error(await r.text());return r.json();
  };
  const settings = await action({action:'global_settings'}), name = 'Renamed Boss ' + Date.now();
  await action({action:'configure_global', boss_name:name, if_version:settings.version});
  await page.evaluate(() => refresh(true));
  check(await input.evaluate(el => el === document.activeElement), 'External global rename preserves comment focus');
  check(await input.inputValue() === 'Keep typing through a global rename', 'External rename preserves comment draft');
  check(await page.locator('#issue-assignment option:checked').innerText() === name, 'External rename updates the assignee automatically');
  check(await input.evaluate(el => el.selectionStart === 5 && el.selectionEnd === 11 && el.selectionDirection === 'backward'), 'Refresh preserves the selected text');
  await page.keyboard.type('writing');
  check(await input.inputValue() === 'Keep writing through a global rename', 'Typing continues at the selection without refocusing');
  await page.getByRole('tab', {name:'Preview'}).click();
  await action({action:'edit', number, title:'Updated from another session', if_version:null, add_labels:[], remove_labels:[]});
  await page.getByRole('heading', {name:'Updated from another session #' + number, exact:true}).waitFor({timeout:12000});
  check(await page.locator('#comment-rendered').isVisible() && (await page.locator('#comment-rendered').innerText()).includes('Keep writing through a global rename'), 'Regular polling updates the title and keeps the draft preview open');
  await page.getByRole('tab', {name:'Write'}).click();
  await input.fill('');await page.evaluate(() => saveComment());
  await action({action:'configure_global', boss_name:settings.boss_name, if_version:settings.version+1});
  return {passed:checks.length,checks};
}
