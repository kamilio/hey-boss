async (page) => {
  const origin = await page.evaluate(() => location.origin),
    reports = [];
  const go = async (id = "") => {
    await page.goto(origin + "/#view=inbox" + (id ? "&notice=" + id : ""));
    await page.waitForFunction(
      (id) =>
        id ? inboxDetail?.taskID === id : document.querySelector(".notice-row"),
      id,
    );
    await page.evaluate(/* AXE_SOURCE */);
  };
  const audit = async (name) => {
    const result = await page.evaluate(async () => {
      const r = await axe.run(document, {
        runOnly: {
          type: "tag",
          values: ["wcag2a", "wcag2aa", "wcag21aa", "best-practice"],
        },
      });
      return r.violations.map((v) => ({
        id: v.id,
        impact: v.impact,
        nodes: v.nodes.map((n) => ({
          target: n.target,
          summary: n.failureSummary,
        })),
      }));
    });
    reports.push({ name, violations: result });
  };
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.emulateMedia({ colorScheme: "light" });
  await go();
  await audit("light-inbox-list");
  await page.screenshot({
    path:
      "output/playwright/inbox-list-light-" +
      (origin.endsWith("4782") ? "chromium" : "webkit") +
      ".png",
  });
  await page.emulateMedia({ colorScheme: "dark" });
  await audit("dark-inbox-list");
  await go("notice-9");
  await audit("pending-question");
  await go("notice-8");
  await audit("pending-review");
  await go("notice-2");
  await audit("answered-question");
  await go("notice-4");
  await audit("review-comments");
  await go("notice-6");
  await audit("linked-notice");
  await page.locator("[data-notice-link]").click();
  await page.waitForSelector(".notice-link-option");
  await audit("link-modal");
  await page.screenshot({
    path:
      "output/playwright/inbox-link-dark-" +
      (origin.endsWith("4782") ? "chromium" : "webkit") +
      ".png",
  });
  await page.keyboard.press("Escape");
  await page.locator(".related-issue-link").click();
  await page.waitForSelector("#detail-view .related-notice-link");
  await page.evaluate(/* AXE_SOURCE */);
  await audit("issue-backlinks");
  await page.setViewportSize({ width: 390, height: 844 });
  await go();
  await audit("mobile-dark-list");
  await go("notice-6");
  await audit("mobile-dark-detail");
  await page.screenshot({
    path:
      "output/playwright/inbox-mobile-dark-" +
      (origin.endsWith("4782") ? "chromium" : "webkit") +
      ".png",
  });
  return reports;
}
