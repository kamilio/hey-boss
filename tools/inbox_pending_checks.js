async (page) => {
  page.on("dialog", dialog => dialog.accept().catch(() => {}));
  const origin = await page.evaluate(() => location.origin),
    checks = [];
  const check = (ok, name) => {
    if (!ok) throw Error(name);
    checks.push(name);
  };
  const go = async (id) => {
    await page.goto(origin + "/#view=inbox&notice=" + id);
    await page.waitForFunction((id) => inboxDetail?.taskID === id, id);
  };
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.emulateMedia({ colorScheme: "light" });
  await go("notice-7");
  await page.locator("#notice-answer").fill("Preserved draft");
  await page.locator("[data-notice-dismiss]").click();
  await page.waitForSelector("#confirm-dialog[open]");
  await page.locator("#confirm-cancel").click();
  check(
    await page.evaluate(() => inboxDetail.status === "pending"),
    "Declining cancellation leaves question pending",
  );
  check(
    (await page.locator("#notice-answer").inputValue()) === "Preserved draft",
    "Declining cancellation preserves answer draft",
  );
  await page.locator("[data-notice-dismiss]").click();
  await page.locator("#confirm-submit").click();
  await page.waitForFunction(() => inboxDetail?.status === "cancelled");
  check(
    (await page.locator(".notice-outcome strong").innerText()) === "Cancelled",
    "Confirmed cancellation is shown separately from an answer",
  );
  await go("notice-8");
  await page.locator("#notice-comment").fill("Unsent review");
  await page.reload();
  await page.waitForSelector("#notice-comment");
  check(
    (await page.locator("#notice-comment").inputValue()) === "Unsent review",
    "Review draft persists across reload",
  );
  await page.locator("#notice-comment").fill("");
  await page.locator("[data-notice-link]").click();
  await page.waitForSelector(".notice-link-option");
  await page.locator('input[value="1"]').check();
  await page.locator("#notice-link-submit").click();
  await page.waitForSelector(".related-issue-link");
  await page.locator(".back-link").click();
  await page.waitForSelector('[data-notice-row="notice-8"] .notice-issue-chip');
  const chip = page.locator('[data-notice-row="notice-8"] .notice-issue-chip');
  check(
    await chip.evaluate((el) => {
      const r = el.getBoundingClientRect(),
        t = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
      return t === el || el.contains(t);
    }),
    "List issue chip receives pointer",
  );
  await chip.click();
  await page.waitForFunction(
    () => model.detail?.issue.number === 1 && model.route.view === "issues",
  );
  await page.waitForSelector("#detail-view .related-notice-link");
  check(
    await page
      .locator("#detail-view .related-notice-link")
      .innerText()
      .then((t) => t.includes("Pending review")),
    "List chip navigates directly to linked issue with backlink",
  );
  return { passed: checks.length, checks };
}
