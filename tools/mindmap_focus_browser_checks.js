async function mindmapFocusChecks(page) {
  let passed = 0;
  const check = (condition, message) => {
    if (!condition) throw new Error(message);
    passed++;
  };
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.reload();
  const ids = await page.evaluate(async () => {
    const boot = await (await fetch("/api/bootstrap")).json();
    const graph = await (
      await fetch("/api/mm", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Hey-Boss-CSRF": boot.csrf,
        },
        body: JSON.stringify({
          project: "named:Atlas",
          operation: {
            action: "mindmap",
            operation: { command: "show", body_mode: "none" },
          },
          request_id: null,
        }),
      })
    ).json();
    return Object.fromEntries(
      graph.nodes
        .filter((node) => ["long-planning", "second-note"].includes(node.alias))
        .map((node) => [node.alias, node.id]),
    );
  });
  check(
    Boolean(ids["long-planning"] && ids["second-note"]),
    "Isolated focus fixture has both long notes",
  );
  const first = ids["long-planning"],
    second = ids["second-note"];
  for (const mode of ["Map", "Outline"]) {
    for (const failed of [false, true]) {
      await page.reload();
      await page.locator("#search").fill("long-planning");
      await page
        .getByRole("button", { name: "Long planning note", exact: true })
        .click();
      if (mode === "Outline")
        await page
          .getByRole("button", { name: "Outline", exact: true })
          .click();
      const scope = page.locator(`[id="${first}"]`);
      let finish;
      const done = new Promise((resolve) => (finish = resolve));
      await page.route("**/api/mm", async (route) => {
        const operation = route.request().postDataJSON().operation.operation;
        if (operation.command !== "view") {
          await route.continue();
          return;
        }
        const response = failed ? null : await route.fetch();
        await page.waitForTimeout(800);
        if (failed)
          await route.fulfill({
            status: 503,
            contentType: "application/json",
            body: JSON.stringify({
              ok: false,
              error: { message: "Synthetic focused read unavailable" },
            }),
          });
        else await route.fulfill({ response });
        finish();
      });
      try {
        await scope.locator("[data-read-body]").click();
        const links = scope.locator(".relationships a");
        check(
          (await links.count()) === 2,
          `${mode}: fixture has two relationships`,
        );
        check(
          (await links.nth(0).getAttribute("href")) ===
            (await links.nth(1).getAttribute("href")),
          `${mode}: relationships share a destination`,
        );
        await links.nth(1).focus();
        const context = await page.evaluate(() =>
          document.activeElement.closest("li").textContent.trim(),
        );
        await done;
        await scope
          .locator(failed ? ".body-error" : "[data-collapse-body]")
          .waitFor();
        const focus = await page.evaluate(() => ({
          tag: document.activeElement.tagName,
          context: document.activeElement.closest("li")?.textContent.trim(),
        }));
        check(
          focus.tag === "A" && focus.context === context,
          `${mode}: ${failed ? "failed" : "successful"} read preserves the specific relationship`,
        );
      } finally {
        await page.unroute("**/api/mm", { behavior: "wait" });
      }
    }
  }
  await page.reload();
  await page.getByRole("button", { name: "Outline", exact: true }).click();
  await page.route("**/api/mm", async (route) => {
    const operation = route.request().postDataJSON().operation.operation;
    if (operation.command !== "view") {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    await page.waitForTimeout(operation.node === first ? 800 : 1800);
    await route.fulfill({ response });
  });
  try {
    await page.locator(`[data-read-body="${first}"]`).click();
    await page.locator(`[data-read-body="${second}"]`).click();
    await page.locator(`[data-collapse-body="${first}"]`).waitFor();
    check(
      await page.evaluate((id) => document.activeElement.id === id, second),
      "Earlier read preserves the newer reading topic",
    );
    await page.locator(`[data-collapse-body="${second}"]`).waitFor();
    check(
      await page.evaluate(
        (id) => document.activeElement.id === `body-${id}`,
        second,
      ),
      "Newer read focuses its loaded body despite intervening rendering",
    );
  } finally {
    await page.unroute("**/api/mm", { behavior: "wait" });
  }
  for (const mode of ["Map", "Outline"]) {
    await page.reload();
    await page.locator("#search").fill("long-planning");
    await page
      .getByRole("button", { name: "Long planning note", exact: true })
      .click();
    if (mode === "Outline")
      await page.getByRole("button", { name: "Outline", exact: true }).click();
    await page.locator(`[data-read-body="${first}"]`).click();
    await page.locator(`[data-collapse-body="${first}"]`).waitFor();
    check(
      await page.evaluate(
        (id) => document.activeElement.id === `body-${id}`,
        first,
      ),
      `${mode}: ordinary full read focuses the body`,
    );
    await page.locator(`[data-collapse-body="${first}"]`).click();
    await page.locator(`[data-read-body="${first}"]`).waitFor();
    check(
      await page.evaluate(
        (id) => document.activeElement.id === `body-${id}`,
        first,
      ),
      `${mode}: ordinary preview read focuses the body`,
    );
  }
  return { passed, linkCases: 4, concurrentReads: 2 };
}
