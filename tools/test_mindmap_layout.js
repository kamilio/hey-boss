const assert = require("node:assert/strict");
const { layout, inViewport } = require("../src/issues/web/mindmap-map.js");
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
