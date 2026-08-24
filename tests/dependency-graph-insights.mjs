import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  FORCE_LAYOUT_NODE_LIMIT,
  graphTopologyProfile,
  minimapGeometry,
  recommendGraphLayout,
} from "../static/dependency-graph-insights-core.js";

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
assert.equal(recommendGraphLayout(chainProfile).recommended, "layered");

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
