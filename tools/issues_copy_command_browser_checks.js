async (page) => {
  await page.goto("http://127.0.0.1:4782/");
  await page.waitForFunction(() => model.csrf && model.project);
  const button = page.getByRole("button", {name: "Copy agent command to create an issue", exact: true});
  await button.waitFor({timeout: 3000});
  const checks = [];
  const check = (ok, name) => {
    if (!ok) throw new Error(name);
    checks.push(name);
  };
  await page.evaluate(() => {
    window.copiedCommands = [];
    Object.defineProperty(navigator, "clipboard", {configurable: true, value: {
      writeText: async (text) => copiedCommands.push(text),
    }});
  });
  const original = await page.evaluate(() => model.project.id);
  await button.click();
  check(await page.evaluate((id) => copiedCommands.at(-1) ===
    `hey-boss issue create --project '${id}' --title '<title>' --body '<markdown>'`, original),
    "Command explicitly targets the full current project ID");
  check(await page.locator("#toast").innerText().then(text => text.includes("Agent command copied")),
    "Successful copying is confirmed");

  const special = "named:Team's project $(touch nope); `echo nope`";
  await page.goto("http://127.0.0.1:4782/#project=" + encodeURIComponent(special));
  await page.waitForFunction(id => model.project.id === id && !model.polling, special);
  await button.click();
  const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
  check(await page.evaluate(() => copiedCommands.at(-1)) ===
    `hey-boss issue create --project ${quote(special)} --title '<title>' --body '<markdown>'`,
    "Switching projects copies the new ID with safe shell quoting");

  await page.evaluate(() => { model.route.host = "dev'box"; });
  await button.click();
  check((await page.evaluate(() => copiedCommands.at(-1))).includes(" --host 'dev'\\''box'"),
    "Remote queue commands explicitly target and quote the selected host");
  await page.evaluate(() => { model.route.host = ""; model.defaultHost = "controller"; });
  await button.click();
  check((await page.evaluate(() => copiedCommands.at(-1))).includes(" --host 'controller'"),
    "A remotely hosted web server includes its default queue host");

  await page.evaluate(() => {
    navigator.clipboard.writeText = async () => { throw new Error("Denied"); };
  });
  await button.click();
  check(await page.locator("#toast").evaluate(el => el.classList.contains("error")),
    "Clipboard rejection reports failure without claiming success");
  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {configurable: true, value: undefined});
    window.originalCopy = document.execCommand.bind(document);
    document.execCommand = command => {
      if (command !== "copy") throw new Error("Unexpected command");
      copiedCommands.push(document.activeElement.value);
      return true;
    };
  });
  await button.click();
  check((await page.evaluate(() => copiedCommands.at(-1))).includes(" --project " + quote(special)),
    "HTTP browsers without Clipboard API can still copy the command");
  check(await page.locator("#toast").evaluate(el => !el.classList.contains("error")),
    "Fallback copy success is confirmed");
  await page.evaluate(() => { document.execCommand = () => false; });
  await button.click();
  check(await page.locator("#toast").evaluate(el => el.classList.contains("error")),
    "Fallback copy failure is reported");
  await page.setViewportSize({width: 390, height: 844});
  const bounds = await button.boundingBox();
  check(bounds.x >= 0 && bounds.x + bounds.width <= 390 && bounds.width <= 40,
    "The small copy button fits the mobile viewport");
  await page.screenshot({path: "output/playwright/issues-copy-command-mobile.png"});
  await page.setViewportSize({width: 1440, height: 1000});
  await page.screenshot({path: "output/playwright/issues-copy-command-desktop.png"});
  return {passed: checks.length, checks};
}
