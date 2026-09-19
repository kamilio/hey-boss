async function mindmapScaleChecks(page) {
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  let passed = 0;
  const check = (condition, message) => {
    if (!condition) throw new Error(message);
    passed++;
  };
  const frame = () =>
    page.evaluate(
      () =>
        new Promise((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(resolve)),
        ),
    );
  const hit = () =>
    page
      .locator(".search-current [data-map-node]")
      .getAttribute("data-map-node");
  const camera = () =>
    page.evaluate(() => {
      const map = document.querySelector("#mindmap"),
        matrix = new DOMMatrix(
          getComputedStyle(document.querySelector(".map-world")).transform,
        );
      return {
        x: (map.clientWidth / 2 - matrix.e) / matrix.a,
        y: (map.clientHeight / 2 - matrix.f) / matrix.a,
        scale: matrix.a,
      };
    });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.reload();
  await page.waitForFunction(
    () =>
      document.querySelector("#count").textContent ===
      "10000 nodes · 20000 links",
  );
  await frame();
  check(
    await page.locator("#search-navigation").isHidden(),
    "Blank search hides result navigation",
  );
  check(
    (await page.locator(".map-item").count()) < 50,
    "Initial cards stay bounded",
  );
  check(
    (await page.locator("#outline .node").count()) === 0,
    "Map avoids outline DOM",
  );
  const initial = await camera();
  await page.locator("#search").fill("topic-987");
  await frame();
  check(
    (await page.locator("#search-status").textContent()) === "1 of 11",
    "Search counts all matching aliases",
  );
  check((await hit()) === "n-scale-987", "Search reveals first match");
  await page.locator("#search").press("Enter");
  check((await hit()) === "n-scale-9870", "Enter reveals next match");
  check(
    await page
      .locator("#search")
      .evaluate((element) => element === document.activeElement),
    "Enter preserves search focus",
  );
  await page.locator("#search").press("Shift+Enter");
  check(
    (await hit()) === "n-scale-987",
    "Shift+Enter returns to previous match",
  );
  await page
    .getByRole("button", { name: "Previous match", exact: true })
    .click();
  check((await hit()) === "n-scale-9879", "Previous match wraps to last match");
  await page.getByRole("button", { name: "Next match", exact: true }).click();
  check((await hit()) === "n-scale-987", "Next match wraps to first match");
  await page
    .locator("#search")
    .dispatchEvent("keydown", { key: "Enter", isComposing: true });
  check(
    (await hit()) === "n-scale-987",
    "IME composition does not navigate results",
  );
  await page
    .getByRole("button", { name: "Planning topic 987", exact: true })
    .click();
  check(
    (await page.locator("#map-details h2").textContent()) ===
      "Planning topic 987",
    "Selecting a match opens native topic details",
  );
  await page.keyboard.press("Escape");
  check(
    await page.locator("#map-inspector").isHidden(),
    "Escape closes details",
  );
  check(
    await page.evaluate(
      () => document.activeElement.dataset.mapNode === "n-scale-987",
    ),
    "Escape returns card focus",
  );
  await page.locator("#search").fill("");
  await frame();
  const restored = await camera();
  check(
    Math.abs(initial.x - restored.x) < 0.01 &&
      Math.abs(initial.y - restored.y) < 0.01,
    "Clearing search restores the viewed area",
  );
  await page.locator("#search").fill("synthetic-missing-topic");
  await frame();
  check(
    (await page.locator("#search-status").textContent()) === "No matches",
    "Empty search result is explicit",
  );
  check(
    await page
      .getByRole("button", { name: "Next match", exact: true })
      .isDisabled(),
    "No matches disables next navigation",
  );
  await page.locator("#search").fill("topic-9999");
  await frame();
  check(
    (await page.locator("#search-status").textContent()) === "1 of 1",
    "Single match count is correct",
  );
  check(
    await page
      .getByRole("button", { name: "Previous match", exact: true })
      .isDisabled(),
    "Single match disables previous navigation",
  );
  await page.locator("#search").fill("topic-987");
  await page.getByRole("button", { name: "Outline", exact: true }).click();
  await page.getByRole("button", { name: "Next match", exact: true }).click();
  check(
    (await page.locator("#outline .highlight").getAttribute("id")) ===
      "n-scale-9870",
    "Outline reveals current search match",
  );
  const searchBox = await page.locator("#search").boundingBox();
  check(
    searchBox.y >= 0 && searchBox.y + searchBox.height <= 900,
    "Outline navigation keeps search visible",
  );
  await page.getByRole("button", { name: "Map", exact: true }).click();
  await page.locator("#search").fill("");
  await frame();
  const beforeResize = await camera();
  await page.locator("#search").fill("topic-987");
  await page.setViewportSize({ width: 320, height: 740 });
  await page.locator("#search").fill("");
  await frame();
  const afterResize = await camera();
  check(
    Math.abs(beforeResize.x - afterResize.x) < 0.01 &&
      Math.abs(beforeResize.y - afterResize.y) < 0.01,
    "Resize during search preserves world center on clearing",
  );
  check(
    (await page.locator(".map-item").count()) > 0,
    "Restored phone map has visible cards",
  );
  await page.locator("#search").fill("topic-987");
  await page.locator("#search").press("Enter");
  check((await hit()) === "n-scale-9870", "Phone search navigates matches");
  check(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth <=
        document.documentElement.clientWidth,
    ),
    "Phone page has no horizontal overflow",
  );
  check(
    (await page.locator("#search").boundingBox()).width > 100,
    "Phone search field stays usable",
  );
  await page.locator("#search").fill("");
  await page.locator("#mindmap").focus();
  await frame();
  const beforePan = await camera();
  await page.keyboard.press("ArrowRight");
  await frame();
  const afterPan = await camera();
  check(afterPan.x > beforePan.x, "Arrow key pans the map");
  await page.keyboard.press("+");
  await frame();
  check((await camera()).scale > afterPan.scale, "Plus key zooms the map");
  check(
    await page.evaluate(() => {
      const controls = document
          .querySelector(".map-controls")
          .getBoundingClientRect(),
        overview = document
          .querySelector("#map-overview")
          .getBoundingClientRect();
      return controls.right <= overview.left;
    }),
    "Narrow phone navigation controls do not overlap the overview",
  );
  // The overview's canvas has padding around it; clicks must target its drawing.
  const overview = await page.locator("#map-overview canvas").boundingBox();
  const world = await page.locator(".map-edges").evaluate((svg) => ({
    width: Number(svg.getAttribute("width")),
    height: Number(svg.getAttribute("height")),
  }));
  await page.locator("#map-overview").evaluate((button) => {
    button.addEventListener("click", (event) => {
      button.dataset.testClick = JSON.stringify({
        x: event.clientX,
        y: event.clientY,
      });
    });
  });
  for (const fraction of [0.1, 0.9]) {
    await page.mouse.click(
      overview.x + overview.width / 2,
      overview.y + overview.height * fraction,
    );
    await frame();
    const centered = await camera();
    // Browsers quantize click coordinates; compare with the event actually sent.
    const click = JSON.parse(
      await page.locator("#map-overview").getAttribute("data-test-click"),
    );
    check(
      Math.abs(
        overview.x + centered.x / world.width * overview.width - click.x,
      ) < 0.01 &&
        Math.abs(
          overview.y + centered.y / world.height * overview.height - click.y,
        ) < 0.01,
      "Overview clicks center the position drawn in its canvas",
    );
  }
  await page.locator("#map-overview").focus();
  await page.keyboard.press("Enter");
  await frame();
  const overviewCenter = await camera();
  check(
    Math.abs(overviewCenter.x - world.width / 2) < 0.1 &&
      Math.abs(overviewCenter.y - world.height / 2) < 0.1,
    "Keyboard overview activation centers the complete map",
  );
  await page.getByRole("link", { name: "Skip to outline", exact: true }).focus();
  await page.keyboard.press("Enter");
  check(
    await page.locator("#outline").isVisible() &&
      await page.locator("#outline").evaluate((element) => element === document.activeElement),
    "Skip link opens and focuses the reading outline",
  );
  check(errors.length === 0, `Browser runtime errors: ${errors.join("; ")}`);
  return {
    passed,
    fixtureNodes: 10000,
    fixtureLinks: 20000,
    mountedCards: await page.locator(".map-item").count(),
  };
}
