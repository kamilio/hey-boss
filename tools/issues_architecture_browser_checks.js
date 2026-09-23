// Run with playwright-cli --session=issue121 run-code --filename this file.
// Start serve_issue_architecture_fixture.mjs first, after building mobile assets.
async page => {
  const errors = [], checks = [];
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  const check = (ok, name) => {if (!ok) throw Error(name); checks.push(name);};
  const mobile = 'http://127.0.0.1:52121';
  const {code} = await (await page.request.get(mobile + '/fixture/pairing')).json();
  check((await page.request.post(mobile + '/api/pair', {data: {code}})).ok(), 'Paired fixture authenticated');
  try {
    for (const [mode, base] of [['desktop', 'http://127.0.0.1:48122/'], ['paired', mobile + '/issue-web/index.html']]) {
      for (const number of [1, 2, 3, 4, 5, 6, 8]) {
        await page.goto(base + '?qa=' + Date.now() + '#project=named%3AIssue%20Design%20QA&issue=' + number);
        await page.locator('.issue-description').waitFor();
        for (const theme of ['light', 'dark']) for (const width of [1440, 768, 390, 320]) {
          await page.setViewportSize({width, height: 960});
          await page.emulateMedia({colorScheme: theme, reducedMotion: 'reduce'});
          const name = `${mode}/${theme}/${width}/issue-${number}`;
          check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), name + ': no horizontal overflow');
          check(await page.locator('.issue-context').evaluate(el => !el.open), name + ': creation details start collapsed');
          check(await page.locator('.issue-description').evaluate(el => el.nextElementSibling.matches('.issue-progress-card')), name + ': description precedes progress');
          check(await page.locator('.issue-work').evaluate(el => !el.textContent.includes('Revision')), name + ': work excludes technical history');
          if (width <= 640) {
            check(await page.evaluate(() => {
              const description = document.querySelector('.issue-description').getBoundingClientRect();
              const progress = document.querySelector('.issue-progress-card').getBoundingClientRect();
              const work = document.querySelector('.issue-work').getBoundingClientRect();
              const discussion = document.querySelector('.issue-discussion').getBoundingClientRect();
              return description.bottom <= progress.top && progress.bottom <= work.top && work.bottom <= discussion.top &&
                document.querySelector('.issue-progress-card').nextElementSibling === document.querySelector('.issue-work');
            }), name + ': phone places work before discussion');
          }
          if (number === 1 && [1440, 390].includes(width)) {
            await page.screenshot({path: `output/playwright/issue121/${mode}-${theme}-${width}.png`, fullPage: true});
          }
        }
      }
      await page.setViewportSize({width: 1440, height: 1000});
      await page.goto(base + '?qa=' + Date.now() + '#project=named%3AIssue%20Design%20QA&issue=1');
      await page.locator('.issue-description').waitFor();
      await page.setViewportSize({width: 390, height: 960});
      check(await page.locator('.issue-work').evaluate(el => el.parentElement.matches('.detail-main')), mode + ': phone reading order matches visible order');
      await page.setViewportSize({width: 1440, height: 1000});
      check(await page.locator('.issue-work').evaluate(el => el.parentElement.matches('.sidebar')), mode + ': resizing restores desktop work column');
      await page.locator('#comment-body').fill('An unsent finding survives opening supporting details.');
      await page.locator('.issue-context > summary').focus();
      await page.keyboard.press('Enter');
      check(await page.locator('[data-issue-version]').isVisible(), mode + ': keyboard reveals revision history');
      await page.locator('.pr-add > summary').focus();
      await page.keyboard.press('Enter');
      check(await page.locator('#pr-url').isVisible(), mode + ': keyboard reveals PR form');
      check(await page.locator('#comment-body').inputValue() === 'An unsent finding survives opening supporting details.', mode + ': disclosures preserve comment draft');
      await page.locator('#history-toggle').click();
      await page.locator('#activity-timeline .timeline-item').first().waitFor();
      check(await page.locator('#history-toggle').getAttribute('aria-expanded') === 'true', mode + ': activity loads in discussion');
      await page.locator('.progress-history summary').click();
      await page.locator('.progress-history-list li').nth(19).waitFor();
      check(await page.locator('.progress-history-more').isVisible(), mode + ': status history pagination works');
      await page.locator('#pr-url').fill('https://github.com/example/hey-boss/pull/' + (mode === 'desktop' ? '43' : '44'));
      await page.locator('#pr-purpose').selectOption('supporting-evidence');
      await page.getByRole('button', {name: 'Attach PR', exact: true}).click();
      await page.waitForFunction(() => !document.querySelector('.pr-add')?.open);
      await page.locator('[data-pr-purpose$="/' + (mode === 'desktop' ? '43' : '44') + '"]').waitFor();
      check(await page.locator('#comment-body').inputValue() === 'An unsent finding survives opening supporting details.', mode + ': linking a PR preserves comment draft');
      await page.locator('#comment-submit').click();
      await page.waitForFunction(() => document.querySelector('#comment-body')?.value === '');
      await page.locator('#comments .comment-body').filter({hasText: 'An unsent finding survives opening supporting details.'}).first().waitFor();
      check(true, mode + ': discussion submits a lasting comment');
      await page.locator('.issue-overflow > summary').click();
      check(await page.locator('[data-action="delete"]').isVisible(), mode + ': delete remains accessible in More actions');
    }
    check(errors.length === 0, 'No browser runtime errors: ' + errors.join(', '));
    return {passed: checks.length, checks: checks.slice(-18)};
  } finally {page.off('pageerror', onError);}
}
