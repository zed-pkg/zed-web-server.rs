import assert from "node:assert/strict";

import {
  ZedDependencyGraph,
  adjacency,
  edgePairIdentity,
  isGraphDigest,
  isStrongGraphEtag,
  packageDocumentUrl,
  packageExportUrl,
  packagePageUrl,
  pathEdgePairs,
  projectionFilename,
  projectionSvgDocument,
  scopeSourceBatches,
} from "../assets/dependency-graph.js";

function sorted(values) {
  return [...values].sort();
}

assert.equal(
  packageDocumentUrl("acme team", "../pkg", "1.2.3+meta"),
  "/bff/dependency-graphs/packages/acme%20team/..%2Fpkg/1.2.3%2Bmeta"
);
assert.equal(
  packageExportUrl("acme team", "pkg/name", "2.0.0-beta.1", "protobuf"),
  "/bff/dependency-graphs/packages/acme%20team/pkg%2Fname/2.0.0-beta.1/export/protobuf"
);
assert.equal(
  packagePageUrl("acme team", "pkg/name"),
  "/p/acme%20team/pkg%2Fname#dependency-graph"
);

const diamondEdges = [
  { from: "root", to: "left" },
  { from: "root", to: "right" },
  { from: "left", to: "leaf" },
  { from: "right", to: "leaf" },
  { from: "root", to: "left" },
];
const diamondOutgoing = adjacency(diamondEdges, "from", "to");
assert.deepEqual(sorted(diamondOutgoing.get("root")), ["left", "right"]);
assert.deepEqual(sorted(diamondOutgoing.get("left")), ["leaf"]);
assert.equal(diamondOutgoing.has("leaf"), false);

assert.notEqual(edgePairIdentity("ab", "c"), edgePairIdentity("a", "bc"));
assert.deepEqual(
  sorted(pathEdgePairs(["root", "left", "leaf"])),
  sorted([
    edgePairIdentity("root", "left"),
    edgePairIdentity("left", "leaf"),
  ])
);
assert.equal(pathEdgePairs(["root"]).size, 0);

assert.equal(isStrongGraphEtag('"graph-json-123"'), true);
assert.equal(isStrongGraphEtag('W/"graph-json-123"'), false);
assert.equal(isStrongGraphEtag("graph-json-123"), false);
assert.equal(isStrongGraphEtag('"bad\nvalue"'), false);
assert.equal(isStrongGraphEtag('""'), true);

assert.equal(isGraphDigest(`sha256:${"a".repeat(64)}`), true);
assert.equal(isGraphDigest(`sha256:${"A".repeat(64)}`), false);
assert.equal(isGraphDigest(`sha256:${"a".repeat(63)}`), false);
assert.equal(isGraphDigest("sha512:not-a-graph-digest"), false);

const scopeBatches = scopeSourceBatches([
  { org: "acme", name: "public-a", private: false },
  { org: "acme", name: "private-b", private: true },
  { org: "acme", name: "public-c" },
]);
assert.deepEqual(
  scopeBatches.publicSources.map(({ source, index }) => [source.name, index]),
  [
    ["public-a", 0],
    ["public-c", 2],
  ]
);
assert.deepEqual(
  scopeBatches.privateSources.map(({ source, index }) => [source.name, index]),
  [["private-b", 1]]
);

const graph = new ZedDependencyGraph();
graph.nodes = new Map(
  ["root", "left", "right", "middle", "leaf", "isolated"].map((id) => [id, { id }])
);
const algorithmEdges = [
  { from: "root", to: "left" },
  { from: "root", to: "right" },
  { from: "left", to: "middle" },
  { from: "middle", to: "leaf" },
  { from: "right", to: "leaf" },
];
const outgoing = adjacency(algorithmEdges, "from", "to");
assert.deepEqual(sorted(graph.walk("root", outgoing)), ["leaf", "left", "middle", "right", "root"]);
assert.deepEqual(graph.shortestPath("root", "leaf", outgoing), ["root", "right", "leaf"]);
assert.deepEqual(graph.shortestPath("root", "isolated", outgoing), []);
assert.deepEqual(graph.shortestPath("root", "root", outgoing), ["root"]);
assert.deepEqual(graph.longestChain("root", outgoing), {
  path: ["root", "left", "middle", "leaf"],
  cyclic: false,
});

const cycleEdges = [
  { from: "a", to: "b" },
  { from: "b", to: "c" },
  { from: "c", to: "a" },
  { from: "c", to: "tail" },
  { from: "self", to: "self" },
  { from: "plain", to: "end" },
];
const cycleOutgoing = adjacency(cycleEdges, "from", "to");
const cycleIncoming = adjacency(cycleEdges, "to", "from");
graph.nodes = new Map(
  ["a", "b", "c", "tail", "self", "plain", "end"].map((id) => [id, { id }])
);
assert.deepEqual(sorted(graph.cycleNodes(cycleOutgoing, cycleIncoming)), ["a", "b", "c", "self"]);
assert.deepEqual(graph.longestChain("a", cycleOutgoing), { path: [], cyclic: true });

function declaredDocument(name, dependencies = []) {
  return {
    schema: "zpkg/dependency-graph/v1",
    view: "declared",
    package: { registry_id: "registry:test", org: "acme", name, version: "1.0.0" },
    dependencies,
  };
}

const fixtures = new ZedDependencyGraph();
fixtures.addDocument(declaredDocument("one"), { primary: true });
assert.equal(fixtures.nodes.size, 1);
assert.equal(fixtures.edges.length, 0);
fixtures.layoutLayered();
assert.deepEqual([...fixtures.positions.values()], [{ x: 0, y: 0 }]);

fixtures.clearGraph();
const duplicate = {
  registry_id: "registry:test",
  org: "acme",
  name: "duplicate",
  requirement: "^1.0.0",
  kind: "runtime",
};
fixtures.addDocument(declaredDocument("root", [duplicate, duplicate]), { primary: true });
assert.equal(fixtures.edges.length, 1);
assert.equal(fixtures.edges[0].count, 2);
fixtures.layoutRadial();
const radialLayout = [...fixtures.positions].map(([id, position]) => [id, { ...position }]);
fixtures.layoutRadial();
assert.deepEqual([...fixtures.positions], radialLayout);
fixtures.layoutForce();
const forceLayout = [...fixtures.positions].map(([id, position]) => [id, { ...position }]);
fixtures.layoutForce();
assert.deepEqual([...fixtures.positions], forceLayout);

fixtures.clearGraph();
const wideDependencies = Array.from({ length: 400 }, (_, index) => ({
  registry_id: "registry:test",
  org: "acme",
  name: `wide-${String(index).padStart(3, "0")}`,
  requirement: "*",
  kind: "runtime",
}));
fixtures.addDocument(declaredDocument("wide-root", wideDependencies), { primary: true });
fixtures.layoutLayered();
const firstLayout = [...fixtures.positions].map(([id, position]) => [id, { ...position }]);
fixtures.layoutLayered();
assert.deepEqual([...fixtures.positions], firstLayout);

const projection = projectionSvgDocument({
  nodes: [...fixtures.nodes.values()],
  edges: fixtures.edges,
  positions: fixtures.positions,
  roots: fixtures.roots,
  selectedId: [...fixtures.roots][0],
  title: "Wide <projection>",
});
assert.ok(projection);
assert.ok(projection.width <= 4096);
assert.ok(projection.height <= 4096);
assert.match(projection.svg, /Wide &lt;projection&gt;/);
assert.match(projection.svg, /400 relationships/);
assert.equal(
  projectionFilename(["acme team", "pkg/name", "1.0.0+build"], "svg"),
  "acme_team_pkg_name_1.0.0+build.dependency-graph.visible.svg"
);

const emptyProjection = projectionSvgDocument({
  nodes: [],
  edges: [],
  positions: new Map(),
});
assert.equal(emptyProjection, null);

const tooManyNodes = new ZedDependencyGraph();
assert.throws(
  () =>
    tooManyNodes.addDocument(
      declaredDocument(
        "bounded-root",
        Array.from({ length: 3000 }, (_, index) => ({
          registry_id: "registry:test",
          org: "acme",
          name: `bounded-${index}`,
        }))
      ),
      { primary: true }
    ),
  /3000-package browser limit/
);
assert.equal(tooManyNodes.nodes.size, 0);

const tooManyEdges = new ZedDependencyGraph();
assert.throws(
  () =>
    tooManyEdges.addDocument({
      schema: "zpkg/dependency-graph/v1",
      view: "resolved",
      nodes: [
        { id: { registry_id: "registry:test", org: "acme", name: "a", version: "1.0.0" } },
        { id: { registry_id: "registry:test", org: "acme", name: "b", version: "1.0.0" } },
      ],
      roots: [],
      edges: Array.from({ length: 12001 }, () => ({
        from: { registry_id: "registry:test", org: "acme", name: "a", version: "1.0.0" },
        to: { registry_id: "registry:test", org: "acme", name: "b", version: "1.0.0" },
      })),
    }),
  /12000-relationship browser limit/
);
assert.equal(tooManyEdges.nodes.size, 0);

console.log("dependency graph workspace tests passed");
