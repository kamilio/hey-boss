async function mindmapRelationshipChecks(page) {
  let passed = 0;
  const check = (condition, message) => {
    if (!condition) throw new Error(message);
    passed++;
  };
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.reload();
  await page.waitForFunction(
    () =>
      document.querySelector("#map-details h2")?.textContent ===
      "Planning topic 987",
  );
  await page.getByRole("button", { name: "Expand all", exact: true }).click();
  await page
    .getByRole("button", { name: "Close topic details", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Planning topic 987", exact: true })
    .click();
  const expected = await page.evaluate(async () => {
    const boot = await (await fetch("/api/bootstrap")).json();
    const graph = await (
      await fetch("/api/mm", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Hey-Boss-CSRF": boot.csrf,
        },
        body: JSON.stringify({
          project: "named:Scale",
          operation: {
            action: "mindmap",
            operation: { command: "show", body_mode: "none" },
          },
          request_id: null,
        }),
      })
    ).json();
    if (!graph.ok) throw new Error("Cannot read isolated relationship fixture");
    return graph.links
      .filter(
        (link) => link.from === "n-scale-987" || link.to === "n-scale-987",
      )
      .map(
        (link) =>
          `${link.from === "n-scale-987" ? link.to : link.from}|${link.description}`,
      );
  });
  check(
    expected.length === 2501,
    "Dense fixture has complete expected relationships",
  );
  check(
    (await page.locator(".map-link").count()) < 50,
    "Offscreen dependencies do not flood SVG",
  );
  const collected = [];
  let pages = 0,
    maxItems = 0,
    maxElements = 0;
  while (true) {
    const current = await page
      .locator("#map-details .relationships li")
      .evaluateAll((items) =>
        items.map((item) => {
          const href = new URL(item.querySelector("a").href),
            id = new URLSearchParams(href.hash.slice(1)).get("node");
          const description = item
            .querySelector(".description")
            .textContent.replace(/^—\s*/, "");
          return `${id}|${description}`;
        }),
      );
    collected.push(...current);
    maxItems = Math.max(maxItems, current.length);
    maxElements = Math.max(
      maxElements,
      await page.locator("#map-details *").count(),
    );
    pages++;
    const next = page.getByRole("button", {
      name: "Next relationships",
      exact: true,
    });
    if (await next.isDisabled()) break;
    if (pages > 60)
      throw new Error("Relationship pagination did not terminate");
    if (pages === 1)
      await page
        .locator("#map-details")
        .evaluate((element) => (element.scrollTop = element.scrollHeight));
    await next.click();
    check(
      (await page.locator("#map-details .relationships li").count()) <= 50,
      "Page stays bounded",
    );
  }
  check(pages === 51, "Every relationship page is reachable");
  check(
    collected.length === expected.length,
    "Pagination retains every relationship",
  );
  check(
    new Set(collected).size === expected.length,
    "Pagination neither skips nor duplicates relationships",
  );
  check(
    [...expected].sort().join("\n") === [...collected].sort().join("\n"),
    "All target IDs and complete descriptions survive paging",
  );
  check(
    maxItems === 50 && maxElements < 300,
    "Inspector DOM remains bounded on every page",
  );
  check(
    await page.evaluate(
      () =>
        document.activeElement.dataset.relPage === "previous" &&
        !document.activeElement.disabled,
    ),
    "Last page leaves usable keyboard focus",
  );
  await page
    .getByRole("button", { name: "Previous relationships", exact: true })
    .click();
  check(
    (await page.locator("#map-details .relationships li").count()) === 50,
    "Previous returns to a full page",
  );
  const pager = await page.locator(".relationship-navigation").boundingBox(),
    details = await page.locator("#map-details").boundingBox();
  check(
    pager.y >= details.y &&
      pager.y + pager.height <= details.y + details.height,
    "Pager stays within the inspector viewport",
  );
  return {
    passed,
    pages,
    relationships: collected.length,
    maxItems,
    maxElements,
  };
}
