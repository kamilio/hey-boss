async function mindmapNativeChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw new Error(message);
    passed++;
  };
  const origin = await page.evaluate(() => location.origin);
  await page.goto(origin + "/mm?focus=1#project=named%3AAtlas");
  await page.reload();
  await page.waitForFunction(() => document.querySelector("#project-name").textContent === "Atlas");
  for (const selector of [".brand", ".app-navigation", ".page-heading", ".page-footer", ".header-right"])
    check(await page.locator(selector).isHidden(), `${selector} is hidden in native focus mode`);
  check(await page.locator("#project-trigger").isVisible(), "Project switcher remains visible");
  check(await page.locator("#refresh").isVisible(), "Refresh remains available for recovery");
  for (const size of [{ width: 1280, height: 900 }, { width: 720, height: 520 }]) {
    await page.setViewportSize(size);
    const bounds = await page.locator("#mindmap").boundingBox();
    check(bounds.height > size.height * 0.6, "Map fills the native window");
    check(await page.evaluate(() => document.documentElement.scrollHeight <= innerHeight), "Focus mode fits without page scrolling");
  }
  await page.locator('#map-fit').click();
  await page.locator('[data-map-node]').first().click();
  const relationship = page.locator('#map-details [data-map-link]').first();
  check((await relationship.getAttribute('href')).startsWith('/mm?focus=1#'), "Map relationships preserve focus mode");
  await relationship.click();
  await page.waitForFunction(() => document.title.startsWith("Beta ·"));
  check(await page.locator(".app-navigation").isHidden(), "Cross-project navigation stays focused");
  await page.locator("#project-trigger").click();
  await page.locator('[data-project="named:Atlas"]').click();
  await page.waitForFunction(() => document.title.startsWith("Atlas ·"));
  await page.locator("#project-trigger").click();
  await page.locator('[data-project="named:Beta"]').click();
  await page.waitForFunction(() => document.title.startsWith("Beta ·"));
  await page.goto(origin + "/mm?focus=1");
  await page.reload();
  await page.waitForFunction(() => document.title.startsWith("Beta ·"));
  check(await page.locator("#project-name").textContent() === "Beta", "Reopening restores the last project without a URL fragment");
  await page.reload();
  await page.waitForFunction(() => document.title.startsWith("Beta ·"));
  check(await page.locator("#project-name").textContent() === "Beta", "Reload preserves the project");
  await page.goto(origin + "/mm#project=named%3AAtlas");
  check(await page.locator(".app-navigation").isVisible(), "Ordinary web viewer retains navigation");
  check(await page.locator(".page-heading").isVisible(), "Ordinary web viewer retains its heading");
  return { passed };
}
