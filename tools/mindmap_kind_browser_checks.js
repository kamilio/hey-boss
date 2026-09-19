async function mindmapKindChecks(page) {
  let passed = 0;
  const check = (value, message) => {
    if (!value) throw new Error(message);
    passed++;
  };
  const kinds = [
    ["text", "Topic", "Release"],
    ["markdown", "Note", "Release notes"],
    ["issue", "Issue", "Fix reconnect"],
    ["pr", "Pull request", "Ship reconnect"],
    ["notification", "Notice", "Review release"],
  ];
  await page.route("**/api/mm", async route => {
    const response = await route.fetch();
    const graph = await response.json();
    graph.nodes = kinds.map(([kind, , title], i) => ({
      id: `kind-${kind}`, kind, title, project_id: graph.project.id,
      parent_id: i ? "kind-text" : null, reference_project: graph.project.id,
      reference: kind === "pr" ? "https://github.com/example/repo/pull/123" : "29",
      state: kind === "issue" ? "open" : null,
      assignee: kind === "issue" ? "human:boss" : null,
      body: "", available: true,
    }));
    graph.external_nodes = []; graph.links = [];
    await route.fulfill({response, json: graph});
  });
  await page.setViewportSize({width: 1280, height: 900});
  await page.reload();
  await page.locator('[data-map-node="kind-text"]').waitFor();
  for (const [kind, label, title] of kinds) {
    await page.locator('#search').fill(title);
    const card = page.locator(`[data-map-node="kind-${kind}"]`);
    await card.waitFor();
    check(await card.locator(`.mindmap-kind-${kind} svg`).count() === 1, `${label} card uses a line icon`);
    check(await card.locator('.mindmap-kind').getAttribute('aria-label') === label, `${label} icon has an accessible name`);
    check(await card.locator('.mindmap-kind').getAttribute('title') === label, `${label} icon has a tooltip`);
    check(!/^(NOTE|PULL REQUEST|PENDING NOTICE|TOPIC)$/.test((await card.innerText()).split('\n').pop()), `${label} card omits the redundant type label`);
    await card.click();
    check(await page.locator(`#map-details .mindmap-kind-${kind}`).count() === 1, `${label} details use the same icon`);
    check(await page.locator('#map-details .badge').count() === 0, `${label} details omit the type badge`);
    await page.getByRole('button', {name: 'Close topic details', exact: true}).click();
  }
  await page.locator('#search').fill('');
  await page.getByRole('button', {name: 'Outline', exact: true}).click();
  for (const [kind, label] of kinds)
    check(await page.locator(`#kind-${kind} > .node-row .mindmap-kind-${kind}`).count() === 1, `${label} outline uses the same icon`);
  check(await page.locator('#outline .badge').count() === 0, 'Outline omits type badges');
  check((await page.locator('#kind-issue .meta').innerText()).includes('open'), 'Issue state stays visible');
  check((await page.locator('#kind-issue .meta').innerText()).includes('Assigned to'), 'Issue assignee stays visible');
  await page.getByRole('button', {name: 'Map', exact: true}).click();
  await page.locator('#search').fill('Ship reconnect');
  const open = page.getByRole('link', {name: 'Open pull request Ship reconnect', exact: true});
  check(await open.getAttribute('href') === 'https://github.com/example/repo/pull/123', 'PR action retains its destination');
  for (const width of [1280, 390]) {
    await page.setViewportSize({width, height: 900});
    await page.getByRole('button', {name: 'Fit', exact: true}).click();
    const card = await page.locator('[data-map-node="kind-pr"]').boundingBox();
    const icon = await page.locator('[data-map-node="kind-pr"] .mindmap-kind').boundingBox();
    const action = await open.boundingBox();
    check(icon.x >= card.x && icon.x + icon.width <= card.x + card.width, `Icon fits the ${width}px card`);
    check(action.x >= card.x && action.x + action.width <= card.x + card.width, `PR action fits the ${width}px card`);
  }
  await page.locator('#search').fill('');
  await page.setViewportSize({width: 1280, height: 900});
  await page.getByRole('button', {name: 'Fit', exact: true}).click();
  return {passed};
}
