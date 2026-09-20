async (page) => {
  await page.goto("http://127.0.0.1:4782/");
  await page.waitForFunction(() => model.csrf && model.project);
  const button = page.getByRole("button", {name: "Copy agent command to create an issue", exact: true});
  await button.waitFor({timeout: 3000});
  const checks = [];
  const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
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
  const original = await page.evaluate(() => model.project.name);
  await button.click();
  check(await page.evaluate(() => copiedCommands.at(-1)) ===
    `hey-boss issue create --project ${quote(original)} --title '<title>' --body '<markdown>'`,
    "Command targets the readable current project name");
  check(await page.locator("#toast").innerText().then(text => text.includes("Agent command copied")),
    "Successful copying is confirmed");
  await button.focus();
  const beforeKeyboard = await page.evaluate(() => copiedCommands.length);
  await page.keyboard.press("Enter");
  check(await page.evaluate(count => copiedCommands.length === count + 1, beforeKeyboard),
    "Keyboard activation copies the command");

  const special = "named:Team's project $(touch nope); `echo nope`";
  await page.goto("http://127.0.0.1:4782/#project=" + encodeURIComponent(special));
  await page.waitForFunction(id => model.project.id === id && !model.polling, special);
  await button.click();
  check(await page.evaluate(() => copiedCommands.at(-1)) ===
    `hey-boss issue create --project ${quote(special.replace(/^named:/, ""))} --title '<title>' --body '<markdown>'`,
    "Switching projects copies the new name with safe shell quoting");

  await page.evaluate(() => {
    model.project = {id: "local:test:/workspace/hey-gh", name: "hey-gh"};
    model.projects = [model.project];
  });
  await button.click();
  check((await page.evaluate(() => copiedCommands.at(-1))).includes(" --project 'hey-gh'"),
    "Local checkout IDs copy only the short project name");
  await page.evaluate(() => {
    model.projects.push({id: "named:hey-gh", name: "hey-gh"});
  });
  await button.click();
  check((await page.evaluate(() => copiedCommands.at(-1))).includes(" --project 'local:test:/workspace/hey-gh'"),
    "Duplicate project names retain an unambiguous full ID");
  await page.evaluate(() => {
    model.projects = [model.project, {id: "hey-gh", name: "Different project"}];
  });
  await button.click();
  check((await page.evaluate(() => copiedCommands.at(-1))).includes(" --project 'local:test:/workspace/hey-gh'"),
    "A name matching another project's ID retains the correct full ID");
  await page.evaluate(id => {
    model.project = {id, name: id.replace(/^named:/, "")};
    model.projects = [model.project];
  }, special);

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
  check((await page.evaluate(() => copiedCommands.at(-1))).includes(" --project " + quote(special.replace(/^named:/, ""))),
    "HTTP browsers without Clipboard API can still copy the command");
  check(await page.locator("#toast").evaluate(el => !el.classList.contains("error")),
    "Fallback copy success is confirmed");
  check(await button.evaluate(el => document.activeElement === el) &&
    await page.locator("textarea[style*='pointer-events']").count() === 0,
    "Fallback copying restores focus and removes its temporary field");
  await page.evaluate(() => { document.execCommand = () => false; });
  await button.click();
  check(await page.locator("#toast").evaluate(el => el.classList.contains("error")),
    "Fallback copy failure is reported");
  await page.setViewportSize({width: 390, height: 844});
  const bounds = await button.boundingBox();
  check(bounds.x >= 0 && bounds.x + bounds.width <= 390 && bounds.width <= 40,
    "The small copy button fits the mobile viewport");
  await page.screenshot({path: "output/playwright/issue62/issues-copy-command-mobile.png"});
  await page.setViewportSize({width: 1440, height: 1000});
  await page.screenshot({path: "output/playwright/issue62/issues-copy-command-desktop.png"});
  return {passed: checks.length, checks};
}
