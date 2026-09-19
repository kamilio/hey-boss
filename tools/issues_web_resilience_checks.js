// Run with playwright-cli run-code --filename, using the isolated review server.
async (page) => {
  const base = "http://127.0.0.1:4782/";
  const project = "github.com/example/hey-boss";
  const checks = [];
  const check = (condition, name) => {
    if (!condition) throw new Error(name);
    checks.push(name);
  };
  await page.goto(base + "#project=" + encodeURIComponent(project));
  await page.reload();
  await page.getByRole("link", { name: "Reconnect automatically after waking from sleep", exact: true }).waitFor();
  let releaseProjects, sawProjects;
  const held = new Promise(resolve => releaseProjects = resolve);
  const received = new Promise(resolve => sawProjects = resolve);
  await page.route("**/api/action", async route => {
    if (route.request().postDataJSON().operation.action === "projects") {
      const response = await route.fetch();
      sawProjects();
      await held;
      await route.fulfill({ response });
    } else await route.continue();
  });
  await page.getByRole("button", { name: "Refresh issues", exact: true }).click();
  await received;
  await page.getByRole("searchbox", { name: "Search issues", exact: true }).fill("reconnect");
  releaseProjects();
  await page.waitForFunction(() => document.querySelectorAll("#issue-list article").length === 1);
  check(await page.locator("#issue-search").inputValue() === "reconnect", "A late background refresh cannot erase text being typed into search");
  await page.unroute("**/api/action");
  await page.locator("#issue-search").fill("");
  await page.waitForFunction(() => document.querySelectorAll("#issue-list article").length === 11);

  let releaseOld, sawOld;
  const oldHeld = new Promise(resolve => releaseOld = resolve);
  const oldReceived = new Promise(resolve => sawOld = resolve);
  await page.route("**/api/action", async route => {
    const data = route.request().postDataJSON();
    if (data.project === "github.com/example/toolcraft" && data.operation.action === "list") {
      const response = await route.fetch();
      sawOld();
      await oldHeld;
      await route.fulfill({ response });
    } else await route.continue();
  });
  await page.evaluate(() => location.hash = "project=github.com%2Fexample%2Ftoolcraft");
  await oldReceived;
  await page.evaluate(() => location.hash = "project=github.com%2Fexample%2Fhey-boss");
  await page.getByRole("link", { name: "Reconnect automatically after waking from sleep", exact: true }).waitFor();
  const oldResponse = page.waitForResponse(response => response.url().endsWith("/api/action") && response.request().postDataJSON().project === "github.com/example/toolcraft" && response.request().postDataJSON().operation.action === "list");
  releaseOld();
  await oldResponse;
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  check(await page.locator("#issue-list article").count() === 11, "A delayed response from another project cannot replace the current list");
  await page.unroute("**/api/action");

  const bootstrap = await (await page.request.get(base + "api/bootstrap")).json();
  const action = async operation => {
    const response = await page.request.post(base + "api/action", { headers: { "X-Hey-Boss-CSRF": bootstrap.csrf }, data: { project: "named:Resilience QA", operation, request_id: null } });
    if (!response.ok()) throw new Error(await response.text());
    return response.json();
  };
  const created = await action({ action: "create", title: "Resilience " + Date.now(), body: "Initial body", labels: [] });
  const number = created.issue.number;
  await page.evaluate(number => location.hash = "project=named%3AResilience%20QA&issue=" + number, number);
  await page.getByRole("textbox", { name: "Your comment", exact: true }).fill("Keep this draft through remote updates");
  await action({ action: "edit", number, title: "Updated in another session", body: null, add_labels: [], remove_labels: [], if_version: null });
  await page.getByRole("button", { name: "Load changes", exact: true }).waitFor({ timeout: 12000 });
  check(await page.locator("#comment-body").inputValue() === "Keep this draft through remote updates", "Polling announces concurrent updates without replacing a comment draft");
  await page.getByRole("button", { name: "Load changes", exact: true }).click();
  await page.getByRole("heading", { name: "Updated in another session #" + number, exact: true }).waitFor();
  check(await page.locator("#comment-body").inputValue() === "Keep this draft through remote updates", "Loading remote changes preserves the comment draft");
  await page.context().setOffline(true);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await page.locator("#comment-error").waitFor();
  check(await page.locator("#comment-body").inputValue() === "Keep this draft through remote updates", "Offline submission preserves its draft and shows a retryable error");
  await page.context().setOffline(false);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#comment-body")?.value === "");
  check(await page.locator("#comments .comment-card").count() === 1, "Retry after reconnect saves exactly one comment");

  await page.emulateMedia({ reducedMotion: "reduce" });
  for (const width of [320, 390, 768, 1024, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "Detail fits width " + width);
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    check(await page.evaluate(() => { const r = document.querySelector("#editor-dialog").getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth && r.bottom <= innerHeight; }), "Editor fits width " + width);
    await page.keyboard.press("Escape");
  }
  check(await page.evaluate(() => document.activeElement !== document.body && document.activeElement.checkVisibility()), "Keyboard focus returns to a visible control after closing dialogs");
  await page.getByRole("button", { name: "All issues", exact: true }).click();
  await page.getByRole("link", { name: "Updated in another session", exact: true }).first().waitFor();
  await page.goto(base + "#project=" + encodeURIComponent(project));
  await page.getByRole("link", { name: "Reconnect automatically after waking from sleep", exact: true }).waitFor();
  return { passed: checks.length, checks };
}
