// Replace AXE_SOURCE with the contents of axe.min.js as a JSON string.
async page => {
  await page.goto('http://127.0.0.1:4793/#project=named%3AChief%20QA',{waitUntil:'domcontentloaded'});
  await page.getByRole('button',{name:'Project settings',exact:true}).click();
  await page.waitForFunction(() => !document.querySelector('#project-chief').disabled);
  await page.locator('#project-chief-instructions summary').click();
  await page.evaluate(/* AXE_SOURCE */);
  const reports = [];
  for (const scheme of ['light','dark']) for (const width of [1440,390,320]) {
    await page.emulateMedia({colorScheme:scheme});
    await page.setViewportSize({width,height:1000});
    await page.locator('.chief-settings').scrollIntoViewIfNeeded();
    const report = await page.evaluate(async () => {
      const result = await axe.run(document,{runOnly:{type:'tag',values:['wcag2a','wcag2aa','wcag21aa','best-practice']}});
      return {passes:result.passes.length,violations:result.violations.map(v => ({id:v.id,nodes:v.nodes.map(n => ({target:n.target,summary:n.failureSummary}))}))};
    });
    reports.push({scheme,width,...report});
  }
  await page.keyboard.press('Escape');
  if (reports.some(r => r.violations.length)) throw Error(JSON.stringify(reports));
  return reports;
}
