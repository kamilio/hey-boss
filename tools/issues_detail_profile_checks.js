// A rename in another session must not interrupt an issue comment.
async page => {
  page.removeAllListeners('dialog');page.on('dialog', d => d.accept().catch(() => {}));
  const origin = await page.evaluate(() => location.origin), checks = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name) };
  await page.goto(origin + '/?detail-profile=' + Date.now());
  await page.waitForFunction(() => model.csrf && model.project);
  const project = await page.evaluate(async () => {
    const value = await api({ action: 'create', title: 'Profile draft QA', body: '', labels: [] }, 'Profile draft QA ' + Date.now());
    await api({ action: 'assign_boss', number: 1, force: false }, value.project.id);
    return value.project.id;
  });
  await page.goto(origin + '/?detail-profile=' + Date.now() + '#project=' + encodeURIComponent(project) + '&issue=1');
  await page.waitForFunction(() => model.detail?.issue.number === 1);
  const input = page.locator('#comment-body');
  await input.fill('Keep typing through a global rename');
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
  check(await page.locator('#update-banner').count() === 1, 'External rename announces fresh issue details');
  await page.keyboard.type(' and continue');
  check(await input.inputValue() === 'Keep typing through a global rename and continue', 'Typing continues without refocusing');
  await page.locator('[data-reload]').click();
  await page.waitForFunction(name => document.querySelector('.assignee-line')?.textContent.includes(name), name);
  check(await input.inputValue() === 'Keep typing through a global rename and continue', 'Loading renamed assignee preserves draft');
  await input.fill('');await page.evaluate(() => saveComment());
  await action({action:'configure_global', boss_name:settings.boss_name, if_version:settings.version+1});
  return {passed:checks.length,checks};
}
