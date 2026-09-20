// Run with playwright-cli run-code against an isolated native artifact server.
async page => {
  const origin = await page.evaluate(() => location.origin);
  const engine = page.context().browser().browserType().name();
  const boot = await (await page.request.get(origin + '/api/bootstrap')).json();
  const checks = [], errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const action = async operation => {
    const response = await page.request.post(origin + '/api/action', {
      headers: {'X-Hey-Boss-CSRF': boot.csrf},
      data: {project: boot.project.id, operation: {action: 'artifact', operation}, request_id: operation.command === 'view' ? null : await page.evaluate(() => crypto.randomUUID())}
    });
    const result = await response.json();
    if (!result.ok) throw Error(JSON.stringify(result));
    return result;
  };
  const created = await action({command: 'create', title: 'Selection comment regression',
    body: '# Review notes\n\nKeep **selected text** close.\n\n' + Array.from({length: 32}, (_, i) =>
      `## Passage ${i + 1}\n\nA thoughtful review preserves the original context and gives the discussion room to grow.`).join('\n\n')});
  const id = created.artifact.id;
  const url = await page.evaluate(({project, id}) => location.origin + '/artifacts#' + new URLSearchParams({project, artifact: id}), {project: boot.project.id, id});
  const visibleComposer = async name => {
    await page.waitForFunction(() => {
      const field = document.querySelector('#artifact-comment'), rect = field.getBoundingClientRect();
      return document.activeElement === field && rect.top >= 0 && rect.bottom <= innerHeight;
    }, null, {timeout: 3000});
    if (await page.locator('#artifact-quote').innerText() !== 'Passage') throw Error(name + ': quote lost');
    if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1)) throw Error(name + ': page overflow');
    checks.push(name + ': quoted composer focused and visible');
  };
  for (const theme of ['light', 'dark']) {
    await page.emulateMedia({colorScheme: theme, reducedMotion: 'reduce'});
    for (const [size, width, height] of [['desktop', 1440, 1000], ['tablet', 820, 1180], ['phone', 390, 844], ['small-phone', 320, 568]]) {
      await page.setViewportSize({width, height});
      await page.goto(url);
      await page.reload();
      await page.locator('#artifact-reading h2').nth(20).waitFor();
      if (await page.locator('#artifact-comments').isVisible()) await page.locator('#artifact-comments-close').click();
      const passage = page.locator('#artifact-reading h2').nth(20);
      await passage.scrollIntoViewIfNeeded();
      const before = await page.locator('#artifact-reading').boundingBox();
      // Select the word using real browser mouse events, rather than inserting a Range.
      await passage.dblclick({position: {x: 35, y: 15}});
      await page.getByRole('button', {name: 'Comment on selection', exact: true}).waitFor();
      if (await page.locator('#artifact-comments').isVisible()) throw Error('Copy selection opened comments');
      const after = await page.locator('#artifact-reading').boundingBox();
      if (before.x !== after.x || before.width !== after.width) throw Error('Copy selection changed reader layout');
      await page.getByRole('button', {name: 'Comment on selection', exact: true}).click();
      await visibleComposer(theme + '-' + size);
      await page.screenshot({path: `output/playwright/issue48/${engine}-${theme}-${size}-composer.png`});
      await page.locator('#artifact-comment').fill('Review ' + theme + '-' + size);
      await page.reload();
      await page.locator('#artifact-quote').waitFor();
      if (await page.locator('#artifact-comment').inputValue() !== 'Review ' + theme + '-' + size) throw Error('Quoted draft lost on reload');
      await page.locator('#artifact-comment-form button[type=submit]').click();
      await page.getByText('Review ' + theme + '-' + size, {exact: true}).waitFor();
      const saved = await action({command: 'view', id});
      const comment = saved.comments.at(-1);
      if (comment.quote !== 'Passage' || comment.outdated) throw Error('Published comment lost its selection anchor');
      checks.push(theme + '-' + size + ': quoted draft survives reload and publishes');
    }
  }
  // Keyboard activation must retain the selected passage after focus leaves the reader.
  await page.setViewportSize({width: 1440, height: 1000});
  await page.goto(url); await page.reload();
  await page.locator('#artifact-comments-toggle').waitFor();
  if (await page.locator('#artifact-comments').isVisible()) await page.locator('#artifact-comments-close').click();
  const passage = page.locator('#artifact-reading h2').nth(20);
  await passage.scrollIntoViewIfNeeded();
  await passage.dblclick({position: {x: 35, y: 15}});
  await page.getByRole('button', {name: 'Comment on selection', exact: true}).focus();
  await page.keyboard.press('Enter');
  await visibleComposer('keyboard-enter');
  await page.locator('#artifact-clear-quote').click();
  if (await page.locator('#artifact-quote').isVisible()) throw Error('Clear selection kept the quote');
  // An already open conversation must work too, including Space activation.
  await passage.scrollIntoViewIfNeeded();
  await passage.dblclick({position: {x: 35, y: 15}});
  await page.getByRole('button', {name: 'Comment on selection', exact: true}).focus();
  await page.keyboard.press('Space');
  await visibleComposer('keyboard-space-comments-open');
  if (errors.length) throw Error(errors.join('\n'));
  return {checks, scriptErrors: errors};
}
