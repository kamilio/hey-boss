async function mindmapShellChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw new Error(message);
    passed++;
  };
  await page.setViewportSize({ width: 1280, height: 900 });
  const origin = await page.evaluate(() => location.origin);
  await page.goto(origin + "/mm#project=named%3AAtlas");
  await page.reload();
  await page.waitForFunction(
    () => document.querySelector("#project-name")?.textContent === "Atlas",
  );
  await page.locator("#search").fill("long-planning");
  const card = page.getByRole("button", {
    name: "Long planning note",
    exact: true,
  });
  const selected = await card.getAttribute("data-map-node");
  await card.click();
  await page.locator("#project-trigger").click();
  check(
    await page.locator("#project-search").evaluate(
      (element) => element === document.activeElement,
    ),
    "Project picker focuses its search",
  );
  await page.keyboard.press("Escape");
  check(
    await page.locator("#project-menu").isHidden(),
    "Escape closes the project picker",
  );
  check(
    await page.locator("#map-inspector").isVisible(),
    "Picker Escape preserves selected topic details",
  );
  check(
    await page.locator("#project-trigger").evaluate(
      (element) => element === document.activeElement,
    ),
    "Picker Escape restores trigger focus",
  );
  await page.keyboard.press("Escape");
  check(
    await page.locator("#map-inspector").isHidden(),
    "Unhandled Escape still closes topic details",
  );
  check(
    await page.evaluate(
      (id) => document.activeElement.dataset.mapNode === id,
      selected,
    ),
    "Topic Escape returns to the selected card",
  );
  return { passed };
}
