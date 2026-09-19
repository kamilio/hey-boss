// Browser-visible paint and list readiness on the isolated review server.
async (page) => {
  const measurements = [];
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("http://127.0.0.1:4782/#project=github.com%2Fexample%2Fhey-boss");
  for (let n = 0; n < 10; n++) {
    await page.reload();
    await page.getByRole("link", { name: "Reconnect automatically after waking from sleep", exact: true }).waitFor();
    measurements.push(await page.evaluate(async () => {
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      return {
        first_contentful_paint_ms: performance.getEntriesByName("first-contentful-paint")[0]?.startTime ?? null,
        list_ready_ms: performance.now(),
        transferred_bytes: performance.getEntriesByType("resource").reduce((n, r) => n + r.transferSize, 0),
      };
    }));
  }
  const summary = {};
  for (const key of ["first_contentful_paint_ms", "list_ready_ms"]) {
    const values = measurements.map(m => m[key]).filter(v => v !== null).sort((a, b) => a - b);
    summary[key] = { median: Math.round((values[4] + values[5]) / 2), max: Math.round(values.at(-1)) };
  }
  return { samples: measurements.length, summary, measurements };
}
