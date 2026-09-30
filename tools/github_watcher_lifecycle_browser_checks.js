// Isolated controller checks; run with playwright-cli run-code --filename.
async page => {
  const checks = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  const fixture = await page.context().newPage();
  await fixture.setContent('<main id="list"><button data-open-watcher="1">First watcher</button><button data-open-watcher="2">Second watcher</button></main>');
  await fixture.addScriptTag({path:'src/issues/web/assignments.js'});
  await fixture.evaluate(() => {
    window.calls = [];
    window.responses = [];
    window.currentHost = 'remote-fixture';
    window.issue = number => ({number,state:'open',assignment:{kind:'github',waiting:true},pull_requests:[{url:`https://github.com/example/repo/pull/${number}`}],github_status:{fetches:{},prs:{}}});
    const options = {
      list:document.querySelector('#list'),
      context:number => ({project:'named:Captured project',host:window.currentHost,issueUrl:'#issue='+number}),
      read:ctx => { calls.push({action:'view',number:ctx.number,project:ctx.project,host:ctx.host}); return new Promise((resolve,reject) => responses.push({resolve,reject})); },
      refresh:ctx => { calls.push({action:'refresh',number:ctx.number,project:ctx.project,host:ctx.host}); return Promise.resolve({issue:issue(ctx.number)}); },
      actorName:actor => actor,bossName:() => 'Boss',icon:() => ''
    };
    IssueAssignments.initWatcher(options);
    IssueAssignments.initWatcher(options);
  });
  check(await fixture.locator('dialog').count() === 1, 'Repeated initialization creates one controller');
  const first = fixture.getByRole('button',{name:'First watcher'});
  const second = fixture.getByRole('button',{name:'Second watcher'});
  const dialog = fixture.getByRole('dialog');
  await first.click();
  await fixture.waitForTimeout(5200);
  check(await fixture.evaluate(() => calls.length === 1), 'Slow requests do not overlap polls');
  await fixture.keyboard.press('Escape');
  await second.click();
  await fixture.evaluate(() => { responses[1].resolve({issue:issue(2)}); currentHost='changed-host'; });
  await dialog.getByText('Awaiting first fetch').waitFor();
  await fixture.evaluate(() => responses[0].resolve({issue:{...issue(1),github_status:{error:'STALE RESPONSE'}}}));
  check(await dialog.getByText('STALE RESPONSE').count() === 0, 'Late response cannot overwrite another watcher');
  await dialog.getByRole('button',{name:'Fetch now'}).click();
  check(await fixture.evaluate(() => { const c=calls.at(-1); return c.action==='refresh' && c.number===2 && c.host==='remote-fixture' && c.project==='named:Captured project'; }), 'Manual fetch uses captured project, host, and issue');
  await fixture.waitForFunction(() => calls.length === 4);
  await fixture.evaluate(() => responses.at(-1).reject(Error('Temporary network failure')));
  await dialog.getByText('Temporary network failure').waitFor();
  check(await dialog.getByText('Awaiting first fetch').isVisible(), 'Read failure preserves last known activity');
  await fixture.waitForFunction(() => calls.length === 5);
  await fixture.evaluate(() => responses.at(-1).resolve({issue:issue(2)}));
  await dialog.getByText('Temporary network failure').waitFor({state:'hidden'});
  check(true, 'Polling recovers from a read failure');
  await dialog.getByRole('button',{name:'Close watcher'}).click();
  const count = await fixture.evaluate(() => calls.length);
  await fixture.waitForTimeout(5200);
  check(await fixture.evaluate(() => calls.length) === count, 'Closing stops polling');
  await first.click();
  await fixture.evaluate(() => location.hash='navigated');
  await dialog.waitFor({state:'hidden'});
  await fixture.evaluate(() => responses.at(-1).resolve({issue:issue(1)}));
  check(!await dialog.isVisible(), 'Navigation closes panel and ignores pending response');
  await fixture.close();
  return {checks:checks.length,passed:checks};
}
