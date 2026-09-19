// Run on a fresh tools/inbox_fixture.swift store and the synthetic Inbox QA issues DB.
async (page) => {
  page.on("dialog", dialog => dialog.accept().catch(() => {}));
  const checks = [],
    errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const check = (ok, name) => {
    if (!ok) throw Error(name);
    checks.push(name);
  };
  const origin = await page.evaluate(() => location.origin);
  await page.reload();
  await page.waitForFunction(() => model.csrf && model.actor);
  const go = async (id = "") => {
    await page.goto(origin + "/#view=inbox" + (id ? "&notice=" + id : ""));
    await page.waitForFunction(
      (id) =>
        model.route.view === "inbox" &&
        (id
          ? inboxDetail?.taskID === id
          : document.querySelectorAll(".notice-row").length > 0),
      id,
    );
  };
  const task = async (id) =>
    page.evaluate(
      (id) => inboxApi({ action: "view", task_id: id }).then((v) => v.task),
      id,
    );
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.emulateMedia({ colorScheme: "light" });
  await go();
  check(
    (await page.locator(".notice-row").count()) === 5,
    "All five unread notices appear without pagination",
  );
  check(
    (await task("notice-1")).status === "pending",
    "Listing does not read notices",
  );
  await page.getByRole("tab", { name: "Activity 1", exact: true }).click();
  await page.waitForSelector('[data-notice-row="notice-6"]');
  check(
    (await page.locator(".notice-row").count()) === 1,
    "Activity contains archived notice",
  );
  await page.getByRole("tab", { name: "Unread 5", exact: true }).click();
  await page.waitForSelector('[data-notice-row="notice-1"]');
  await page
    .getByRole("combobox", { name: "Filter Inbox by project" })
    .selectOption("Other project");
  await page.waitForFunction(
    () => document.querySelectorAll(".notice-row").length === 1,
  );
  check(
    (await page.locator(".notice-title").innerText()) === "Deployment complete",
    "Project filter works",
  );
  await go();
  await page.getByRole("searchbox", { name: "Search Inbox" }).fill("Publish");
  await page.waitForFunction(
    () => document.querySelectorAll(".notice-row").length === 1,
  );
  check(
    await page
      .locator("#inbox-search")
      .evaluate((el) => el === document.activeElement),
    "Search keeps keyboard focus",
  );
  await go("notice-2");
  check(
    (await task("notice-2")).status === "pending",
    "Viewing question does not answer or read it",
  );
  const before = await page.evaluate(() =>
    api({ action: "view", number: 1 }, "named:Inbox QA").then((v) => v.issue),
  );
  await page.getByRole("button", { name: "Link issue", exact: true }).click();
  await page.waitForSelector(".notice-link-option");
  check(
    await page.locator("#notice-link-project").inputValue() === "named:Inbox QA",
    "Link picker starts on the notice's matching project",
  );
  check(
    (await page.locator(".notice-link-option").count()) === 2,
    "Link picker includes open and closed issues",
  );
  await page.locator('input[name="related_issue"][value="1"]').check();
  await page.locator("#notice-link-submit").click();
  await page.waitForSelector(".related-issue-link");
  const linked = await task("notice-2");
  check(
    linked.issue.project === "named:Inbox QA" &&
      linked.issue.number === 1 &&
      linked.status === "pending",
    "Link persists without completing question",
  );
  const after = await page.evaluate(() =>
    api({ action: "view", number: 1 }, "named:Inbox QA").then((v) => v.issue),
  );
  check(
    JSON.stringify(before) === JSON.stringify(after),
    "Link leaves issue content, revision, state and assignment untouched",
  );
  await page.locator(".related-issue-link").click();
  await page.waitForSelector(".related-notice-link");
  check(
    await page.locator("#issue-heading").isHidden(),
    "Issue detail hides list heading after Inbox navigation",
  );
  check(
    await page
      .locator(".related-notice-link")
      .innerText()
      .then((s) => s.includes("Publish release?")),
    "Issue shows notice backlink",
  );
  await page.locator(".related-notice-link").click();
  await page.waitForSelector('[data-notice-answer="Approve"]');
  await page.reload();
  await page.waitForSelector(".related-issue-link");
  check(
    (await task("notice-2")).status === "pending",
    "Relationship survives reload without changing status",
  );
  await page.getByRole("button", { name: "Unlink", exact: true }).click();
  await page.waitForFunction(() => !inboxDetail.issue);
  check(!(await task("notice-2")).issue, "Unlink removes relationship");
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await page.waitForFunction(() => inboxDetail?.status !== "pending");
  check(
    (await task("notice-2")).result === "Approve",
    "Approval answer uses native completion",
  );
  const winner = await page.evaluate(() =>
    inboxApi({ action: "respond", task_id: "notice-2", answer: "Reject" }),
  );
  check(
    winner.changed === false && winner.task.result === "Approve",
    "Late answer cannot replace winning answer",
  );
  await go("notice-3");
  await page.locator("#notice-answer").fill("Use the durable queue");
  await go();
  await go("notice-3");
  check(
    (await page.locator("#notice-answer").inputValue()) ===
      "Use the durable queue",
    "Answer draft survives navigation",
  );
  await page.locator("#notice-answer").focus();
  await page.keyboard.press("Control+Enter");
  await page.waitForFunction(() => inboxDetail?.status !== "pending");
  check(
    (await task("notice-3")).result === "Use the durable queue",
    "Keyboard shortcut submits Inbox answer",
  );
  await go("notice-1");
  await page.waitForFunction(() => inboxDetail?.status !== "pending");
  check(
    (await page.locator(".markdown table").count()) === 1 &&
      (await page.locator(".markdown strong").count()) > 0,
    "Full Markdown tables and emphasis render",
  );
  check(
    !(await page.evaluate(() => window.injected)) &&
      (await page.locator(".markdown script").count()) === 0,
    "Notice Markdown strips executable HTML",
  );
  check(
    (await task("notice-1")).status === "ok",
    "Ordinary update is marked read on view",
  );
  await go("notice-4");
  check(
    (await task("notice-4")).status === "pending",
    "Review remains pending on view",
  );
  await page.locator("#notice-comment").fill("**Looks good**, verified.");
  await page.getByRole("button", { name: "Add comment", exact: true }).click();
  await page.waitForFunction(() => inboxDetail?.comments?.length === 1);
  check(
    (await page.locator(".notice-comments .markdown strong").innerText()) ===
      "Looks good",
    "Review comment renders Markdown",
  );
  check(
    (await task("notice-4")).status === "pending",
    "Comment does not finish review",
  );
  await page
    .getByRole("button", { name: "Finish review", exact: true })
    .click();
  await page.waitForFunction(() => inboxDetail?.status !== "pending");
  check(
    (await task("notice-4")).status === "ok",
    "Finish review completes native request",
  );
  await go("notice-6");
  await page.getByRole("button", { name: "Link issue", exact: true }).click();
  await page.waitForSelector(".notice-link-option");
  await page.locator('input[name="related_issue"][value="2"]').check();
  await page.locator("#notice-link-submit").click();
  await page.waitForSelector(".related-issue-link");
  check(
    (await task("notice-6")).status === "ok",
    "Archived notice links to closed issue without lifecycle change",
  );
  await page.locator(".related-issue-link").click();
  await page.waitForSelector(".related-notice-link");
  check(
    (await page.locator("#detail-view .state-pill").first().innerText()) ===
      "Closed",
    "Linked closed issue opens directly",
  );
  await go();
  await page.locator("#nav-issues").click();
  await page.waitForSelector(".issue-row");
  check(
    await page.locator("#issue-heading").isVisible(),
    "Issues heading restores from Inbox",
  );
  await go();
  await page.setViewportSize({ width: 390, height: 844 });
  check(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    "Mobile Inbox has no horizontal overflow",
  );
  await page.emulateMedia({ colorScheme: "dark" });
  check(
    await page.evaluate(
      () => matchMedia("(prefers-color-scheme: dark)").matches,
    ),
    "Dark appearance follows system",
  );
  await go("notice-6");
  await page.getByRole("button", { name: "Change link", exact: true }).click();
  await page.waitForSelector(".notice-link-option");
  check(
    await page.locator("#notice-link-dialog").evaluate((el) => {
      const r = el.getBoundingClientRect();
      return (
        r.left >= 0 &&
        r.right <= innerWidth &&
        r.top >= 0 &&
        r.bottom <= innerHeight
      );
    }),
    "Link modal fits mobile viewport",
  );
  await page.keyboard.press("Escape");
  check(
    await page.locator("#notice-link-dialog").evaluate((el) => !el.open),
    "Escape closes link modal",
  );
  check(errors.length === 0, "No JavaScript runtime errors");
  return { passed: checks.length, checks };
}
