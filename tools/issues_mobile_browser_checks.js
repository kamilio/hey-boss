// Run with playwright-cli run-code --filename tools/issues_mobile_browser_checks.js.
// Start an isolated web server on 4783 with --mobile-origin https://mac.example.ts.net:8443.
// Routes simulate the TLS proxy, preserving Host/Origin, without a live tailnet.
async (page) => {
  page.setDefaultTimeout(10000);
  page.setDefaultNavigationTimeout(10000);
  const origin = "https://mac.example.ts.net:8443";
  const local = "http://127.0.0.1:4783";
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const check = (condition, name) => {
    if (!condition) throw new Error(name);
  };
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await page.route(origin + "/**", async (route) => {
    const request = route.request();
    const reply = await page.request.fetch(local + request.url().slice(origin.length), {
      method: request.method(),
      headers: { ...request.headers(), host: "mac.example.ts.net:8443" },
      data: request.postDataBuffer() ?? undefined,
    });
    await route.fulfill({ response: reply });
  });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(origin);
  await page.waitForFunction(() => model.csrf && model.project && model.signature);
  await page.locator("#new-issue").click();
  await page.locator("#editor-subject").fill("Phone regression " + Date.now());
  await page.locator("#editor-body").fill("Synthetic private draft");
  check(await page.locator("#mobile-draft-help").isVisible(), "Mobile draft hint is visible");
  check(await page.evaluate(() => localStorage.length === 0 && sessionStorage.length === 0), "Mobile draft is not persisted");
  await page.locator("#editor-close").click();
  await page.locator("#new-issue").click();
  check(await page.locator("#editor-body").inputValue() === "Synthetic private draft", "Draft survives same-page navigation");
  await page.reload();
  await page.waitForFunction(() => model.csrf && model.project && model.signature);
  await page.locator("#new-issue").click();
  check(await page.locator("#editor-body").inputValue() === "", "Reload discards mobile draft");
  const title = "Phone issue " + Date.now();
  await page.locator("#editor-subject").fill(title);
  await page.locator("#editor-body").fill("# Phone Markdown\n\nSynthetic authoritative content.");
  await page.locator("#editor-submit").click();
  await page.waitForFunction(() => model.detail?.issue.title.startsWith("Phone issue "));
  const number = await page.evaluate(() => model.detail.issue.number);
  await page.locator("#comment-body").fill("Phone comment");
  await page.locator("#comment-submit").click();
  await page.waitForFunction(() => model.detail.comments.some((comment) => comment.body === "Phone comment"));
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "Phone issue fits the viewport");
  check(await page.evaluate(() => localStorage.length === 0 && sessionStorage.length === 0), "Saved issue and retry payloads are not persisted on phone");
  await page.goto(local + "/#project=named%3Amobile-test&issue=" + number);
  await page.waitForFunction(() => model.detail?.issue.title.startsWith("Phone issue "));
  check(await page.locator("#detail-view h1").textContent().then((text) => text.includes(title)), "Desktop sees the phone's saved issue");
  await page.locator("#comment-body").fill("Desktop draft");
  check(await page.evaluate(() => Object.values(localStorage).some((value) => value.includes("Desktop draft"))), "Desktop keeps durable drafts");
  check(errors.length === 0, "No browser script errors: " + errors.join(", "));
  console.log("Passed: mobile create/comment, shared store, phone layout, memory-only drafts and retries, reload discard, desktop draft persistence.");
}
