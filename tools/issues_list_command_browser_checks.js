// Run with playwright-cli run-code against an isolated server on port 4782.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:4782/');
  await page.waitForFunction(() => model.csrf && model.project && !model.polling);
  const button = page.getByRole('button', {name: 'Copy command to retrieve the filtered issue list', exact: true});
  await button.waitFor({timeout: 3000});
  await page.evaluate(() => {
    window.copiedCommands = [];
    Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {
      writeText: async text => copiedCommands.push(text),
    }});
  });
  const copy = async () => { await button.click(); return page.evaluate(() => copiedCommands.at(-1)); };
  check(await copy() === "hey-boss issue list --project 'List QA' --all --state open", 'Default command retrieves every open issue');
  const filters = '#project=named%3AList+QA&state=closed&owner=unassigned&label=ready&search=Matching';
  await page.goto('http://127.0.0.1:4782/' + filters);
  await page.waitForFunction(() => model.route.state === 'closed' && !model.polling);
  check(await copy() === "hey-boss issue list --project 'List QA' --all --state closed --unassigned --label 'ready' --search 'Matching'", 'State, assignee, label, and search follow the current filters');
  await page.evaluate(() => { model.route.owner = 'mine'; });
  check((await copy()).includes("--assignee 'human:boss'") && !(await copy()).includes('--mine'), 'Assigned to me preserves the web Boss identity in agent terminals');
  await page.evaluate(() => { model.route.owner = "codex:agent's session"; model.route.host = "dev'box"; document.querySelector('#issue-search').value = "Team's $(touch nope); `echo nope`"; });
  const quoted = await copy();
  check(quoted.includes("--host 'dev'\\''box'") && quoted.includes("--assignee 'codex:agent'\\''s session'") && quoted.includes("--search 'Team'\\''s $(touch nope); `echo nope`'"), 'Host, owner, and shell metacharacters are quoted safely');
  await page.evaluate(() => { model.route.host = ''; model.defaultHost = 'controller'; model.projects.push({id: 'named:Other', name: 'List QA'}); });
  check((await copy()).includes("--project 'named:List QA' --host 'controller'"), 'Ambiguous names use exact IDs and default remote hosts are preserved');
  await button.focus();
  const before = await page.evaluate(() => copiedCommands.length);
  await page.keyboard.press('Enter');
  check(await page.evaluate(count => copiedCommands.length === count + 1, before), 'Keyboard activation copies the command');
  await page.evaluate(() => { navigator.clipboard.writeText = async () => { throw Error('Denied'); }; });
  await button.click();
  check(await page.locator('#toast').evaluate(el => el.classList.contains('error')), 'Clipboard denial reports an actionable error');
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {configurable: true, value: undefined});
    document.execCommand = () => { copiedCommands.push(document.activeElement.value); return true; };
  });
  await button.click();
  check(await button.evaluate(el => document.activeElement === el) && await page.locator('textarea[style*="pointer-events"]').count() === 0, 'HTTP fallback restores keyboard focus and removes its temporary field');
  check(await page.locator('#toast').innerText().then(text => text.includes('Issue list command copied')), 'Copy success is announced');
  await page.evaluate(() => { document.execCommand = () => false; });
  await button.click();
  check(await page.locator('#toast').evaluate(el => el.classList.contains('error')), 'HTTP fallback failure reports an error');
  await page.evaluate(() => { model.defaultHost = ''; model.route.host = ''; });
  await page.goto('http://127.0.0.1:4782/');
  await page.waitForFunction(() => model.csrf && !model.polling);
  await page.locator('.issue-row').first().waitFor();
  for (const scheme of ['light', 'dark']) {
    await page.emulateMedia({colorScheme: scheme});
    for (const width of [1440, 768, 390, 320]) {
      await page.setViewportSize({width, height: 900});
      const bounds = await button.boundingBox();
      check(bounds && bounds.x >= 0 && bounds.x + bounds.width <= width && bounds.width >= 28 && bounds.width <= 40, `${scheme} ${width}px: compact copy button stays visible`);
      check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${scheme} ${width}px: no horizontal overflow`);
      if (width === 320) check(await page.locator('.state-tabs').evaluate(el => el.scrollWidth <= el.clientWidth), `${scheme} 320px: all four state tabs fit beside the copy button`);
      await page.screenshot({path: `output/playwright/issue91/list-command-${scheme}-${width}.png`});
    }
  }
  check(errors.length === 0, 'No browser JavaScript errors');
  return {passed: checks.length, checks};
}
