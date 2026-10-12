// Run against serve_project_settings_fixture.mjs with playwright-cli run-code --filename.
async (page) => {
  let checks = 0;
  const check = (ok, message) => { if (!ok) throw Error(message); checks++; };
  const names = ['poe-code', 'A very long project name with spaces and a useful ending', 'project_'.repeat(30), '日本語のプロジェクト名'];
  for (const theme of ['light', 'dark']) for (const width of [1280, 768, 390, 320]) {
    await page.emulateMedia({colorScheme: theme});
    await page.setViewportSize({width, height: 800});
    for (const name of names) {
      await page.evaluate(name => { model.project.name = name; }, name);
      await page.locator('#project-settings-trigger').click();
      await page.locator('#project-prompt').waitFor({state: 'visible'});
      const heading = page.locator('#project-settings-title');
      check((await heading.innerText()).includes(name), 'Headline includes the full project name');
      check(await page.locator('#project-settings-name').getAttribute('title') === name, 'Truncated name has a full-name tooltip');
      check(await heading.evaluate(el => {
        const name = el.querySelector('#project-settings-name');
        const title = el.getBoundingClientRect(), project = name.getBoundingClientRect();
        const close = document.querySelector('#project-settings-close').getBoundingClientRect();
        return project.top >= title.top && project.bottom <= title.bottom + 1
          && title.height < 35 && title.right <= close.left && project.width >= 40;
      }), `One headline with room for project and close at ${width}px`);
      check(await page.locator('#project-settings-dialog').evaluate(el => el.scrollWidth <= el.clientWidth + 1), 'No horizontal overflow');
      check(await page.locator('#project-settings-close').evaluate(el => {
        const r = el.getBoundingClientRect();
        return r.left >= 0 && r.right <= innerWidth && r.top >= 0;
      }), 'Close remains visible');
      await page.getByRole('tab', {name: 'Workflow', exact: true}).click();
      check(await heading.isVisible(), 'Headline remains visible across tabs');
      await page.locator('#project-settings-close').click();
      check(await page.locator('#project-settings-trigger').evaluate(el => el === document.activeElement), 'Closing restores focus');
    }
  }
  console.log(`COMPLETE: ${checks}/${checks} heading checks`);
  return {passed: checks};
}
