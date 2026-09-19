// Run with playwright-cli run-code against an isolated issue web server on 4793.
async page => {
  const checks = [], errors = [], signals = [];
  const check = (ok, name) => { if (!ok) throw new Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {writeText: async text => { window.copiedSession = text; }}});
  });
  const now = Date.now();
  const run = {id: 'active', project_name: 'poe2', number: 32, title: 'Fix reconnect', state: 'running', started_at: now - 120000, finished_at: null, session_id: 'private-session-id', last_event: 'Investigating reconnect', goal: {status: 'paused'}};
  const history = {...run, id: 'history', number: 31, state: 'claim_timeout', goal: {status: 'paused'}, started_at: now - 300000, finished_at: now - 180000, session_id: 'history-session-id'};
  const worker = {id: 'live', pid: 123, config: {name: 'poe2 worker', concurrency: 2, enabled: true, projects: ['github.com/kamilio/poe2']}, runs: [run, history]};
  const data = {machines: [
    {host: 'local', hostname: 'MacBook', role: 'supervisor', state: 'connected', workers: [worker, {...worker, id: 'saved', pid: null, config: {...worker.config, name: 'Saved worker'}, runs: []}]},
    {host: 'remote', hostname: 'Devbox', state: 'disconnected', heartbeat: now / 1000 - 60, workers: [{...worker, id: 'offline'}]},
  ]};
  await page.route('**/api/fleet/status', route => {
    data.machines[0].heartbeat = Date.now() / 1000;
    return route.fulfill({json: data});
  });
  await page.route('**/api/fleet/events', route => route.fulfill({contentType: 'text/event-stream', body: ': fixture\n\n'}));
  await page.route('**/api/fleet', async route => { signals.push(route.request().postDataJSON()); await route.fulfill({json: {ok: true}}); });
  await page.setViewportSize({width: 1440, height: 1000});
  await page.goto('http://127.0.0.1:4793/workers');
  await page.waitForSelector('.worker-tree .worker');
  check((await page.locator('#overview').innerText()).includes('1\nRunning workers\n1\nActive agents\n2\nAgent capacity'), 'Counts include only live workers and agents');
  check(await page.locator('.worker-tree>.worker').count() === 1, 'Supervisor contains live worker, which contains agent');
  check(!(await page.locator('#machines').innerText()).includes('private-session-id'), 'Session IDs hidden');
  check((await page.locator('.agents').first().innerText()).includes('Goal: paused'), 'Goal state visible');
  check(!(await page.locator('details[data-section=saved]').evaluate(el => el.open)), 'Saved definitions collapsed');
  check(!(await page.locator('details[data-section=offline]').evaluate(el => el.open)), 'Disconnected snapshots collapsed');
  const copy = page.locator('.worker-tree .agents button[data-session]').first();
  await copy.click();
  check(await page.evaluate(() => window.copiedSession) === 'private-session-id', 'Copy button copies full session ID');
  check((await page.locator('#copy-status').textContent()) === 'Session ID copied.', 'Copy result announced');
  const duration = await page.locator('.agents .duration').first().textContent();
  await page.waitForFunction(before => document.querySelector('.agents .duration').textContent !== before, duration);
  check(true, 'Active elapsed time ticks without replacing controls');
  await page.locator('.history>summary').first().click();
  check((await page.locator('.history').first().innerText()).includes('claim_timeout') && (await page.locator('.history .duration').first().textContent()) === '2m00s', 'History shows final state and duration');
  await page.locator('.history button[data-session]').first().click();
  check(await page.evaluate(() => window.copiedSession) === 'history-session-id', 'History sessions can be copied');
  await page.locator('#refresh').click();
  await page.waitForFunction(() => document.querySelector('.history').open);
  check(await page.locator('.history').first().evaluate(el => el.open), 'Refresh preserves expanded history');
  await copy.focus();
  await page.waitForTimeout(5500);
  check(await page.locator('.worker-tree .agents button[data-session]').first().evaluate(el => el === document.activeElement), 'Automatic refresh preserves button focus');
  await page.locator('.worker-tree button[data-signal=pause]').click();
  check(signals.length === 1 && signals[0].host === 'local' && signals[0].worker === 'live' && signals[0].signal === 'pause', 'Worker controls target the correct host and worker');
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {value: undefined});
    document.execCommand = command => { window.fallbackSession = document.querySelector('.clipboard-input').value; return command === 'copy'; };
  });
  await copy.click();
  check(await page.evaluate(() => window.fallbackSession) === 'private-session-id' && await page.locator('.clipboard-input').count() === 0, 'HTTP clipboard fallback copies and cleans up');
  await page.screenshot({path: 'output/playwright/issue13-workers-desktop.png'});
  await page.setViewportSize({width: 390, height: 844});
  await page.locator('details[data-section=offline]>summary').click();
  const frozen = await page.locator('details[data-section=offline] .agents .duration').first().textContent();
  await page.waitForTimeout(1100);
  check((await page.locator('details[data-section=offline] .agents .duration').first().textContent()) === frozen, 'Offline durations stop at last heartbeat');
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Mobile view fits viewport');
  await page.emulateMedia({colorScheme: 'dark'});
  await page.screenshot({path: 'output/playwright/issue13-workers-mobile-dark.png'});
  check(errors.length === 0, 'No browser errors');
  return {passed: checks.length, checks};
}
