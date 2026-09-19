const assert = require("node:assert/strict");
const {
  layout,
  inViewport,
  indexGraph,
} = require("../src/issues/web/mindmap-map.js");
const node = (id, parent_id = null) => ({ id, parent_id, title: id });
const fixture = [
  node("a"),
  node("a1", "a"),
  node("a2", "a"),
  node("b"),
  node("b1", "b"),
];
const full = layout(fixture, new Set());
const byId = new Map(full.items.map((n) => [n.id, n]));
assert.equal(full.items.length, 5);
assert(byId.get("a1").x > byId.get("a").x);
assert(byId.get("a1").y < byId.get("a2").y);
assert(byId.get("a2").y + 84 < byId.get("b1").y);
assert.equal(byId.get("a").color, byId.get("a1").color);
assert.notEqual(byId.get("a").color, byId.get("b").color);
assert.deepEqual(
  layout(fixture, new Set(["a"])).items.map((n) => n.id),
  ["a", "b", "b1"],
);
assert.deepEqual(
  layout(fixture, new Set(["a"]), new Set(["a", "a2"])).items.map((n) => n.id),
  ["a", "a2"],
);
assert.equal(layout([]).items.length, 0);
assert.equal(layout([node("orphan", "missing")]).items.length, 1);
assert(inViewport({ x: 0, y: 0 }, { x: 10, y: 10, width: 100, height: 100 }));
assert(
  !inViewport({ x: 1000, y: 1000 }, { x: 10, y: 10, width: 100, height: 100 }),
);
const searchable = [
  { id: "release", title: "Release", kind: "text", alias: "launch" },
  {
    id: "api",
    title: "Shared API",
    kind: "issue",
    state: "open",
    body: "Café 🧭 planning",
    reference: "42",
    reference_project: "named:Platform",
    reference_project_name: "Platform",
    assignee: "human:boss",
  },
];
const dependencies = [
  { from: "release", to: "api", kind: "depends-on", description: "API first" },
];
const indexed = indexGraph(searchable, dependencies, () => "Morgan");
for (const query of ["shared api", "depends-on", "api first"])
  assert(indexed.documents.get("release").includes(query));
for (const query of [
  "café",
  "🧭",
  "platform",
  "42",
  "open",
  "morgan",
  "human:boss",
])
  assert(indexed.documents.get("api").includes(query));
assert(indexed.documents.get("release").includes("launch"));
assert.equal(indexed.nodes.get("api"), searchable[1]);
assert.equal(indexed.incidents.get("api")[0], dependencies[0]);
const fresh = indexGraph(
  [
    searchable[0],
    { ...searchable[1], title: "Renamed API", body: "Full loaded text" },
  ],
  dependencies,
  () => "Avery",
);
assert(fresh.documents.get("release").includes("renamed api"));
assert(!fresh.documents.get("release").includes("shared api"));
assert(fresh.documents.get("api").includes("full loaded text"));
assert(fresh.documents.get("api").includes("avery"));
const unavailable = indexGraph(
  [{ ...searchable[1], available: false }],
  [
    {
      from: "api",
      to: "missing",
      kind: "related",
      description: "Resource removed",
    },
  ],
  () => "Morgan",
);
assert(!unavailable.documents.get("api").includes("morgan"));
assert(unavailable.documents.get("api").includes("resource removed"));
assert.equal(indexGraph([], [], () => "").documents.size, 0);
const big = Array.from({ length: 10000 }, (_, i) =>
  node(`n${i}`, i < 25 ? null : `n${i % 25}`),
);
const start = performance.now();
const large = layout(big, new Set(big.slice(0, 25).map((n) => n.id)));
assert.equal(large.items.length, 25);
assert(large.items.every((n) => Number.isFinite(n.x) && Number.isFinite(n.y)));
console.log(
  JSON.stringify({
    ok: true,
    fixtureNodes: 10000,
    collapsedCards: 25,
    layoutMilliseconds: performance.now() - start,
  }),
);
