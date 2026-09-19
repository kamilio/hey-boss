// Read-only against an isolated devbox issue; relationship mutations touch only a synthetic notice store.
async (page) => {
  const origin = await page.evaluate(() => location.origin),
    project = "named:Inbox remote QA",
    checks = [];
  const check = (ok, name) => {
    if (!ok) throw Error(name);
    checks.push(name);
  };
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(origin + "/#view=inbox&notice=notice-9");
  await page.waitForFunction(() => inboxDetail?.taskID === "notice-9");
  const before = await page.evaluate(
    (project) =>
      api({ action: "view", number: 1 }, project, null, "qa-devbox").then(
        (v) => v.issue,
      ),
    project,
  );
  await page.locator("[data-notice-link]").click();
  await page.waitForSelector(".notice-link-option");
  await page.locator("#notice-link-host").fill("qa-devbox");
  await page.locator("#notice-link-host").press("Tab");
  await page.waitForFunction(
    (project) =>
      [...document.querySelector("#notice-link-project").options].some(
        (o) => o.value === project,
      ),
    project,
  );
  await page.locator("#notice-link-project").selectOption(project);
  await page.waitForFunction(() =>
    noticeLinkIssues.some((i) => i.number === 1),
  );
  await page.locator('input[name="related_issue"][value="1"]').check();
  await page.locator("#notice-link-submit").click();
  await page.waitForSelector(".related-issue-link");
  check(
    await page.evaluate(
      () =>
        inboxDetail.issue.host === "qa-devbox" &&
        inboxDetail.status === "pending",
    ),
    "Remote host relationship persists without completing notice",
  );
  await page.locator(".related-issue-link").click();
  await page.waitForFunction(
    () =>
      model.route.host === "qa-devbox" &&
      model.route.issue === 1 &&
      model.detail?.issue.number === 1,
  );
  await page.waitForSelector("#detail-view .related-notice-link");
  check(
    await page
      .locator("#detail-view .related-notice-link")
      .innerText()
      .then((t) => t.includes("Answer needed")),
    "Remote issue shows local Inbox backlink",
  );
  const after = await page.evaluate(
    (project) =>
      api({ action: "view", number: 1 }, project, null, "qa-devbox").then(
        (v) => v.issue,
      ),
    project,
  );
  check(
    JSON.stringify(before) === JSON.stringify(after),
    "Remote issue content and revision remain unchanged",
  );
  await page.locator("#detail-view .related-notice-link").click();
  await page.waitForSelector("#notice-answer");
  check(
    await page.evaluate(
      () =>
        inboxDetail.taskID === "notice-9" &&
        inboxDetail.issue.host === "qa-devbox",
    ),
    "Remote issue backlink returns to local notice",
  );
  return { passed: checks.length, checks };
}
