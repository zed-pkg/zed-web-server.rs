import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  FORCE_LAYOUT_NODE_LIMIT,
  PROFILE_MAX_EDGES,
  PROFILE_MAX_NODES,
  PROFILE_MAX_TEXT_BYTES,
  graphTopologyProfile,
  minimapGeometry,
  recommendGraphLayout,
} from "../static/dependency-graph-insights-core.js";

assert.equal(PROFILE_MAX_NODES, 3000);
assert.equal(PROFILE_MAX_EDGES, 12000);
assert.equal(PROFILE_MAX_TEXT_BYTES, 2048);

function nodes(count) {
  return new Map(Array.from({ length: count }, (_, index) => [`n${index}`, { id: `n${index}` }]));
}

const chainEdges = Array.from({ length: 8 }, (_, index) => ({
  from: `n${index}`,
  to: `n${index + 1}`,
}));
const chainProfile = graphTopologyProfile(nodes(9), chainEdges, new Set(["n0"]));
assert.equal(chainProfile.maxDepth, 8);
assert.equal(chainProfile.cycleRatio, 0);
assert.equal(chainProfile.stronglyConnectedComponentCount, 9);
assert.equal(recommendGraphLayout(chainProfile).recommended, "layered");

const shortcutProfile = graphTopologyProfile(
  nodes(5),
  [
    { from: "n0", to: "n1" },
    { from: "n1", to: "n2" },
    { from: "n2", to: "n3" },
    { from: "n0", to: "n3" },
  ],
  new Set(["n0"])
);
assert.equal(shortcutProfile.maxDepth, 3, "depth is longest condensation path, not BFS distance");

const hubEdges = Array.from({ length: 18 }, (_, index) => ({
  from: "n0",
  to: `n${index + 1}`,
}));
const hubProfile = graphTopologyProfile(nodes(19), hubEdges, new Set(["n0"]));
assert.ok(hubProfile.hubRatio > 0.9);
assert.equal(recommendGraphLayout(hubProfile).recommended, "radial");

const cycleEdges = Array.from({ length: 10 }, (_, index) => ({
  from: `n${index}`,
  to: `n${(index + 1) % 10}`,
}));
cycleEdges.push(
  { from: "n0", to: "n5" },
  { from: "n2", to: "n7" },
  { from: "n4", to: "n9" }
);
const cycleProfile = graphTopologyProfile(nodes(10), cycleEdges, new Set(["n0"]));
assert.ok(cycleProfile.cycleRatio > 0.8);
assert.equal(recommendGraphLayout(cycleProfile).recommended, "force");

const cycleWithTailProfile = graphTopologyProfile(
  nodes(5),
  [
    { from: "n0", to: "n1" },
    { from: "n1", to: "n0" },
    { from: "n1", to: "n2" },
    { from: "n2", to: "n3" },
    { from: "n3", to: "n4" },
  ],
  new Set(["n0"])
);
assert.equal(cycleWithTailProfile.cyclicNodeCount, 2);
assert.equal(cycleWithTailProfile.cycleRatio, 0.4, "nodes downstream of a cycle are not cyclic");
assert.equal(cycleWithTailProfile.maxDepth, 3);

const selfLoopProfile = graphTopologyProfile(
  nodes(2),
  [{ from: "n0", to: "n0" }],
  new Set(["n0"])
);
assert.equal(selfLoopProfile.cyclicNodeCount, 1);
assert.equal(selfLoopProfile.cycleRatio, 0.5);

const duplicateProfile = graphTopologyProfile(
  nodes(2),
  [
    { from: "n0", to: "n1" },
    { from: "n0", to: "n1", kind: "runtime", optional: false },
  ],
  new Set(["n0"])
);
assert.equal(duplicateProfile.edgeCount, 1, "identical relationships are profiled once");
assert.equal(duplicateProfile.maxDegree, 1);

assert.throws(
  () => graphTopologyProfile([{ id: "duplicate" }, { id: "duplicate" }], []),
  /duplicate node identifiers/
);
assert.throws(
  () => graphTopologyProfile(nodes(2), [{ from: "n0", to: "unknown" }]),
  /invalid relationship/
);
assert.throws(
  () => graphTopologyProfile(nodes(2), [{ from: "n0", to: "n1", optional: "yes" }]),
  /invalid relationship/
);
assert.throws(
  () => graphTopologyProfile(new Map([["😀".repeat(513), {}]]), []),
  /invalid node identifier/
);

const duplicateRootsProfile = graphTopologyProfile(
  nodes(2),
  [{ from: "n0", to: "n1" }],
  ["n0", "n0"]
);
assert.equal(duplicateRootsProfile.rootCount, 1);

const disconnectedProfile = graphTopologyProfile(
  nodes(4),
  [{ from: "n0", to: "n1" }, { from: "n2", to: "n3" }],
  new Set(["n0", "n2"])
);
assert.equal(disconnectedProfile.componentCount, 2);
assert.equal(disconnectedProfile.maxWidth, 2);

assert.throws(
  () => graphTopologyProfile(nodes(3001), []),
  /3000-node contract limit/
);
assert.throws(
  () => graphTopologyProfile(nodes(2), Array.from({ length: 12001 }, () => null)),
  /12000-edge contract limit/
);

const largeProfile = graphTopologyProfile(
  nodes(FORCE_LAYOUT_NODE_LIMIT + 1),
  Array.from({ length: FORCE_LAYOUT_NODE_LIMIT }, (_, index) => ({
    from: `n${index}`,
    to: `n${index + 1}`,
  })),
  new Set(["n0"])
);
const largeRecommendation = recommendGraphLayout(largeProfile);
assert.notEqual(largeRecommendation.recommended, "force");
assert.equal(largeRecommendation.scores.force, 0);

const geometry = minimapGeometry(
  new Map([
    ["left", { x: -100, y: -50 }],
    ["middle", { x: 0, y: 0 }],
    ["right", { x: 100, y: 50 }],
  ]),
  { x: 50, y: 30, k: 2 },
  { width: 800, height: 600 },
  { width: 196, height: 118, padding: 10 }
);
assert.equal(geometry.points.size, 3);
assert.ok(geometry.points.get("left").x < geometry.points.get("right").x);
assert.ok(geometry.viewport.width >= 1 && geometry.viewport.width <= 196);
assert.ok(geometry.viewport.height >= 1 && geometry.viewport.height <= 118);
assert.ok(geometry.viewport.x + geometry.viewport.width <= 196);
assert.ok(geometry.viewport.y + geometry.viewport.height <= 118);
const clippedGeometry = minimapGeometry(
  new Map([
    ["left", { x: -100, y: -50 }],
    ["right", { x: 100, y: 50 }],
  ]),
  { x: -10000, y: -10000, k: 2 },
  { width: 20, height: 20 },
  { width: 196, height: 118, padding: 10 }
);
assert.ok(clippedGeometry.viewport.x + clippedGeometry.viewport.width <= 196);
assert.ok(clippedGeometry.viewport.y + clippedGeometry.viewport.height <= 118);
const center = geometry.worldAt(geometry.points.get("middle").x, geometry.points.get("middle").y);
assert.ok(Math.abs(center.x) < 1e-9);
assert.ok(Math.abs(center.y) < 1e-9);

const pluginSource = readFileSync(
  new URL("../static/dependency-graph-insights.js", import.meta.url),
  "utf8"
);
const styleSource = readFileSync(
  new URL("../static/dependency-graph-insights.css", import.meta.url),
  "utf8"
);
assert.ok(pluginSource.includes('"/graph-assets/dependency-graph.js"'));
assert.ok(pluginSource.includes("recommendGraphLayout"));
assert.ok(pluginSource.includes("renderMinimap"));
assert.ok(!pluginSource.includes("fetch("), "insights module must not fetch an external runtime");
assert.ok(!/import\s+(?:[^;]+from\s+)?["']https?:\/\//.test(pluginSource));
assert.ok(styleSource.includes(".dg-visual-search"));
assert.ok(styleSource.includes(".dg-minimap"));
assert.ok(styleSource.includes("prefers-reduced-motion"));
const pluginModule = await import(
  new URL("../static/dependency-graph-insights.js", import.meta.url)
);
assert.equal(typeof pluginModule.updateVisualSearch, "function");
assert.equal(typeof pluginModule.renderMinimap, "function");

console.log("dependency graph visual-search tests passed");
