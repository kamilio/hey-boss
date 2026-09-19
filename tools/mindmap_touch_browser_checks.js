async function mindmapTouchChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw new Error(message);
    passed++;
  };
  await page.setViewportSize({ width: 390, height: 844 });
  const origin = await page.evaluate(() => location.origin);
  await page.goto(origin + "/mm#project=named%3AAtlas");
  await page.reload();
  await page.waitForFunction(
    () => document.querySelector("#project-name")?.textContent === "Atlas",
  );
  await page.locator("#search").fill("release");
  await page.locator("#mindmap").scrollIntoViewIfNeeded();
  const card = page.getByRole("button", { name: "Autumn release", exact: true });
  const center = async () => {
    const box = await card.boundingBox();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  };
  const camera = () => page.evaluate(() => {
    const matrix = new DOMMatrix(document.querySelector(".map-world").style.transform);
    return { x: matrix.e, y: matrix.f, scale: matrix.a };
  });
  const cdp = await page.context().newCDPSession(page);
  const touch = async (type, touchPoints) => {
    await cdp.send("Input.dispatchTouchEvent", { type, touchPoints });
    await page.waitForTimeout(35);
  };
  try {
    await cdp.send("Input.dispatchTouchEvent", { type: "touchCancel", touchPoints: [] }).catch(() => {});
    let point = await center();
    const before = await camera();
    await touch("touchStart", [{ id: 1, ...point }]);
    for (let step = 1; step <= 6; step++) {
      await touch("touchMove", [{ id: 1, x: point.x + step * 12, y: point.y + step * 4 }]);
      const moved = await camera();
      check(
        Math.abs(moved.x - before.x - step * 12) < 0.25 &&
          Math.abs(moved.y - before.y - step * 4) < 0.25,
        "Every touch move from a card continues panning",
      );
    }
    await touch("touchEnd", []);
    check(await page.locator("#map-inspector").isHidden(), "Dragging from a card does not select it");
    point = await center();
    const initialScale = (await camera()).scale;
    const fingers = (distance) => [{ id: 1, x: point.x - distance, y: point.y }, { id: 2, x: point.x + distance, y: point.y }];
    await touch("touchStart", fingers(30));
    for (let step = 1; step <= 6; step++) {
      await touch("touchMove", fingers(30 + step * 6));
      check(
        Math.abs((await camera()).scale - Math.min(2, initialScale * (30 + step * 6) / 30)) < 0.01,
        "Every two-finger move from a card continues zooming",
      );
    }
    await touch("touchEnd", []);
    const map = await page.locator("#mindmap").boundingBox();
    await touch("touchStart", [{ id: 3, x: map.x + 15, y: map.y + 15 }]);
    await touch("touchMove", [{ id: 3, x: map.x + 29, y: map.y + 21 }]);
    await touch("touchCancel", []);
    check(!(await page.locator("#mindmap").getAttribute("class") || "").includes("panning"), "Cancellation clears the panning state");
    point = await center();
    await touch("touchStart", [{ id: 4, ...point }]);
    await touch("touchEnd", []);
    check(await page.locator("#map-inspector").isVisible(), "A fresh tap still selects the card after cancellation");
    return { passed, dragSteps: 6, pinchSteps: 6 };
  } finally {
    await cdp.send("Input.dispatchTouchEvent", { type: "touchCancel", touchPoints: [] }).catch(() => {});
    await cdp.detach();
  }
}
