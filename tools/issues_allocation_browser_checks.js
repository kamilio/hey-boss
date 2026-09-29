// Run against an isolated issue server seeded with a machine assignment.
async page => {
  const checks = [], errors = [];
  const check = (ok, name) => { if (!ok) throw Error(name); checks.push(name); };
  page.on('pageerror', error => errors.push(error.message));
  await page.waitForSelector('.issue-assignment');
  const original = await page.evaluate(() => structuredClone(model.detail));
  const assignment = original.issue.assignment;
  check(assignment.kind === 'machine', 'The real machine destination reaches issue details');
  const select = page.getByLabel('Assignment', {exact:true});
  check(await select.inputValue() === 'machine:' + assignment.machine, 'One control shows the assigned machine');
  check(await page.getByRole('button',{name:'Release reservation',exact:true}).count() === 0, 'No separate allocation control remains');
  await select.focus();
  check(await select.evaluate(node => node === document.activeElement), 'Assignment supports keyboard focus');
  for (const issue of [{draft:true},{state:'blocked'},{state:'closed'}]) {
    await page.evaluate(({original,issue}) => renderDetail({...original,issue:{...original.issue,...issue}}), {original,issue});
    check(await select.isDisabled(), 'Paused or closed work cannot be reassigned: ' + JSON.stringify(issue));
  }
  await page.evaluate(original => renderDetail(original), original);
  await page.setViewportSize({width:390,height:844});
  await select.scrollIntoViewIfNeeded();
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'The assignment fits a phone screen');
  let request;
  page.on('request', value => {
    if (value.url().endsWith('/api/action')) {
      const body = value.postDataJSON();
      if (body.operation?.action === 'assign') request = body;
    }
  });
  await select.selectOption('unassigned');
  await page.waitForFunction(() => model.detail?.issue?.assignment?.kind === 'unassigned');
  check(request.operation.target === 'unassigned' && request.operation.if_version === original.issue.version && !!request.request_id, 'Destination changes carry revision and retry guards');
  const state = await page.evaluate(async () => api({action:'view',number:model.detail.issue.number}));
  check(state.allocation.reserved_machine === null, 'Clearing assignment releases the real reservation');
  await select.selectOption('machine:' + assignment.machine);
  await page.waitForFunction(machine => model.detail?.issue?.assignment?.machine === machine, assignment.machine);
  check((await select.inputValue()) === 'machine:' + assignment.machine, 'The machine can be assigned again');
  check(errors.length === 0, 'No browser errors');
  return {passed:checks.length,checks};
}
