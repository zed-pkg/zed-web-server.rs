import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  ZedDependencyGraph,
  adjacency,
  assertDeclaredDocumentCoordinate,
  cacheControlDisallowsStorage,
  edgeIdentity,
  edgePairIdentity,
  escapeHtml,
  isGraphDigest,
  isStrongGraphEtag,
  packageDocumentUrl,
  packageExportUrl,
  packagePageUrl,
  parseContentLength,
  pathEdgePairs,
  projectionFilename,
  projectionSvgDocument,
  scopeSourceBatches,
} from "../assets/dependency-graph.js";

const digest = `sha256:${"a".repeat(64)}`;
const graphStyles = readFileSync(
  new URL("../assets/dependency-graph.css", import.meta.url),
  "utf8"
);

assert.match(graphStyles, /\.dg-toolbar\s*\{\s*z-index:\s*3;\s*\}/);
assert.match(graphStyles, /\.dg-querybar\s*\{\s*z-index:\s*2;\s*\}/);

function sorted(values) {
  return [...values].sort();
}

function graphResponse({
  body,
  status = 200,
  cacheControl = "public, max-age=31536000, immutable",
  etag = '"representation-bytes"',
  authoritative = "true",
  selectedVersion = "",
  contentLength,
}) {
  const bytes = new TextEncoder().encode(JSON.stringify(body));
  const values = new Map(
    Object.entries({
      "cache-control": cacheControl,
      "content-length": String(contentLength ?? bytes.byteLength),
      "content-type": "application/vnd.zpkg.dependency-graph.v1+json",
      etag,
      "x-zpkg-graph-authoritative": authoritative,
      "x-zpkg-graph-digest": body.graph_digest,
      "x-zpkg-selected-version": selectedVersion,
    })
  );
  return {
    status,
    ok: status >= 200 && status < 300,
    headers: { get: (name) => values.get(name.toLowerCase()) ?? null },
    arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
    json: async () => body,
  };
}

assert.equal(
  packageDocumentUrl("acme tools", "http/client", "2.0.0-beta.1+build.7"),
  "/bff/dependency-graphs/packages/acme%20tools/http%2Fclient/2.0.0-beta.1%2Bbuild.7"
);
assert.equal(
  packageExportUrl("acme", "http", "2.0.0-beta.1", "messagepack"),
  "/bff/dependency-graphs/packages/acme/http/2.0.0-beta.1/export/messagepack"
);
assert.equal(
  packageExportUrl("acme team", "pkg/name", "2.0.0-beta.1", "protobuf"),
  "/bff/dependency-graphs/packages/acme%20team/pkg%2Fname/2.0.0-beta.1/export/protobuf"
);
assert.equal(packagePageUrl("acme", "http/client"), "/p/acme/http%2Fclient#dependency-graph");
assert.equal(
  packagePageUrl("acme team", "pkg/name"),
  "/p/acme%20team/pkg%2Fname#dependency-graph"
);
assert.ok(packageDocumentUrl("acme", "http", "1.0.0").startsWith("/"));
assert.ok(!packageDocumentUrl("acme", "http", "1.0.0").startsWith("//"));
assert.equal(
  packageDocumentUrl("acme team", "../pkg", "1.2.3+meta"),
  "/bff/dependency-graphs/packages/acme%20team/..%2Fpkg/1.2.3%2Bmeta"
);

assert.ok(isStrongGraphEtag('"representation-bytes"'));
assert.ok(!isStrongGraphEtag('W/"representation-bytes"'));
assert.ok(!isStrongGraphEtag("representation-bytes"));
assert.ok(!isStrongGraphEtag('"bad\nvalue"'));
assert.ok(isStrongGraphEtag('""'));
assert.ok(isGraphDigest(digest));
assert.ok(!isGraphDigest(`sha256:${"A".repeat(64)}`));
assert.ok(!isGraphDigest(`sha256:${"a".repeat(63)}`));
assert.ok(!isGraphDigest("sha512:not-a-graph-digest"));
assert.equal(parseContentLength("4096"), 4096);
assert.equal(parseContentLength("4KiB"), null);
assert.equal(parseContentLength(null), null);
assert.ok(cacheControlDisallowsStorage("private, NO-STORE"));
assert.ok(!cacheControlDisallowsStorage("public, max-age=60"));
assert.equal(
  escapeHtml(`<img src=x onerror='alert(1)'>&\"`),
  "&lt;img src=x onerror=&#39;alert(1)&#39;&gt;&amp;&quot;"
);

const sources = [
  { org: "acme", name: "public", version: "1.0.0", private: false },
  { org: "acme", name: "private", version: "1.0.0", private: true },
  { org: "acme", name: "public-two", version: "2.0.0", private: false },
];
const batches = scopeSourceBatches(sources);
assert.deepEqual(
  batches.publicSources.map(({ index }) => index),
  [0, 2]
);
assert.deepEqual(
  batches.privateSources.map(({ index }) => index),
  [1]
);

const edges = [
  { from: "a", to: "b" },
  { from: "a", to: "c" },
  { from: "b", to: "c" },
];
assert.deepEqual([...adjacency(edges, "from", "to").get("a")], ["b", "c"]);
assert.deepEqual([...pathEdgePairs(["a", "b", "c"])], ["a\0b", "b\0c"]);
assert.notEqual(edgePairIdentity("ab", "c"), edgePairIdentity("a", "bc"));
assert.notEqual(
  edgeIdentity({
    from: "a\0b",
    to: "c",
    kind: "runtime",
    requirement: "",
    target: "",
    optional: false,
  }),
  edgeIdentity({
    from: "a",
    to: "b\0c",
    kind: "runtime",
    requirement: "",
    target: "",
    optional: false,
  })
);
assert.deepEqual(
  sorted(pathEdgePairs(["root", "left", "leaf"])),
  sorted([
    edgePairIdentity("root", "left"),
    edgePairIdentity("left", "leaf"),
  ])
);
assert.equal(pathEdgePairs(["root"]).size, 0);

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

const graph = new ZedDependencyGraph();
const document = {
  schema: "zpkg/dependency-graph/v1",
  view: "declared",
  graph_digest: digest,
  package: {
    registry_id: "https://registry.zpkg.net",
    org: "acme",
    name: "http",
    version: "2.0.0-beta.1",
  },
  dependencies: [
    {
      registry_id: "https://registry.zpkg.net",
      org: "acme",
      name: "tls",
      requirement: "^1.0.0",
      kind: "runtime",
      optional: false,
      features: ["rustls"],
    },
  ],
};
assert.doesNotThrow(() =>
  assertDeclaredDocumentCoordinate(document, "acme", "http", "2.0.0-beta.1")
);
assert.throws(
  () => assertDeclaredDocumentCoordinate(document, "acme", "http", "2.0.0"),
  /requested exact package version/
);
graph.addDocument(document, { primary: true });
assert.equal(graph.nodes.size, 2);
assert.equal(graph.edges.length, 1);
assert.equal(graph.roots.size, 1);
graph.addDocument(document, { primary: true });
assert.equal(graph.edges.length, 1, "identical relationships are coalesced");
assert.equal(graph.edges[0].count, 2);
graph.layoutLayered();
const rootId = [...graph.roots][0];
const dependencyId = [...graph.nodes.keys()].find((id) => id !== rootId);
graph.positions.set(dependencyId, { x: 100_000, y: 100_000 });
const projection = projectionSvgDocument({
  nodes: [...graph.nodes.values()],
  edges: graph.edges,
  positions: graph.positions,
  roots: graph.roots,
  selectedId: rootId,
  title: "Exact <projection>",
});
assert.ok(projection);
assert.ok(projection.width <= 4096);
assert.ok(projection.height <= 4096);
assert.match(projection.svg, /Exact &lt;projection&gt;/);
assert.match(projection.svg, /2 packages and 1 relationships/);
assert.equal(
  projectionFilename(["acme team", "pkg/name", "1.0.0+build"], "svg"),
  "acme_team_pkg_name_1.0.0+build.dependency-graph.visible.svg"
);
assert.equal(
  projectionSvgDocument({ nodes: [], edges: [], positions: new Map() }),
  null
);
assert.throws(
  () => graph.addDocument({ ...document, schema: "unknown" }),
  /unsupported dependency graph schema/
);
const malformed = new ZedDependencyGraph();
assert.throws(
  () => malformed.addDocument({ ...document, dependencies: [null] }),
  /invalid package identity/
);
assert.equal(malformed.nodes.size, 0, "shape failures do not leave a partial graph");
assert.throws(
  () =>
    new ZedDependencyGraph().addDocument({
      ...document,
      dependencies: [{ ...document.dependencies[0], name: "bad\ud800name" }],
    }),
  /invalid package name/
);
assert.throws(
  () =>
    new ZedDependencyGraph().addDocument({
      ...document,
      dependencies: Array.from({ length: 3000 }, () => document.dependencies[0]),
    }),
  /3000-package browser limit/
);

const boundedEdgeGraph = new ZedDependencyGraph();
const resolvedA = {
  registry_id: "registry:test",
  org: "acme",
  name: "a",
  version: "1.0.0",
};
const resolvedB = { ...resolvedA, name: "b" };
assert.throws(
  () =>
    boundedEdgeGraph.addDocument({
      schema: "zpkg/dependency-graph/v1",
      view: "resolved",
      nodes: [{ id: resolvedA }, { id: resolvedB }],
      roots: [],
      edges: Array.from({ length: 12_001 }, () => ({
        from: resolvedA,
        to: resolvedB,
        kind: "runtime",
      })),
    }),
  /12000-relationship browser limit/
);
assert.equal(boundedEdgeGraph.nodes.size, 0, "edge-cap failures remain atomic");

const algorithmGraph = new ZedDependencyGraph();
algorithmGraph.nodes = new Map(
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
assert.deepEqual(sorted(algorithmGraph.walk("root", outgoing)), [
  "leaf",
  "left",
  "middle",
  "right",
  "root",
]);
assert.deepEqual(algorithmGraph.shortestPath("root", "leaf", outgoing), [
  "root",
  "right",
  "leaf",
]);
assert.deepEqual(algorithmGraph.shortestPath("root", "isolated", outgoing), []);
assert.deepEqual(algorithmGraph.shortestPath("root", "root", outgoing), ["root"]);
assert.deepEqual(algorithmGraph.longestChain("root", outgoing), {
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
algorithmGraph.nodes = new Map(
  ["a", "b", "c", "tail", "self", "plain", "end"].map((id) => [id, { id }])
);
assert.deepEqual(sorted(algorithmGraph.cycleNodes(cycleOutgoing, cycleIncoming)), [
  "a",
  "b",
  "c",
  "self",
]);
assert.deepEqual(algorithmGraph.longestChain("a", cycleOutgoing), { path: [], cyclic: true });

const originalFetch = globalThis.fetch;
try {
  const cacheGraph = new ZedDependencyGraph();
  const requests = [];
  globalThis.fetch = async (url, options) => {
    requests.push({ url, options });
    return graphResponse({ body: document, selectedVersion: "2.0.0-beta.1" });
  };
  const first = await cacheGraph.fetchDocumentNow("public-graph");
  assert.equal(first.document.package.name, "http");
  assert.equal(first.authoritative, "true");
  assert.ok(first.contentLength > 0);
  assert.ok(cacheGraph.cache.has("public-graph"));

  globalThis.fetch = async (url, options) => {
    requests.push({ url, options });
    return graphResponse({
      body: document,
      status: 304,
      selectedVersion: "2.0.0-beta.1",
    });
  };
  const revalidated = await cacheGraph.fetchDocumentNow("public-graph");
  assert.equal(revalidated.document, first.document);
  assert.equal(requests.at(-1).options.headers["If-None-Match"], '"representation-bytes"');

  globalThis.fetch = async (_url, options) => {
    requests.push({ url: "private-graph", options });
    return graphResponse({ body: document, cacheControl: "private, no-store" });
  };
  await cacheGraph.fetchDocumentNow("private-graph", { noStore: true });
  assert.equal(requests.at(-1).options.cache, "no-store");
  assert.equal(requests.at(-1).options.headers["If-None-Match"], undefined);
  assert.ok(!cacheGraph.cache.has("private-graph"));

  cacheGraph.rememberDocument("visibility-race", first);
  let visibilityRequest = 0;
  globalThis.fetch = async (_url, options) => {
    visibilityRequest += 1;
    requests.push({ url: "visibility-race", options });
    return graphResponse({
      body: document,
      status: visibilityRequest === 1 ? 304 : 200,
      cacheControl: visibilityRequest === 1 ? "private, no-store" : "public, max-age=60",
      selectedVersion: "2.0.0-beta.1",
    });
  };
  await cacheGraph.fetchDocumentNow("visibility-race");
  assert.equal(visibilityRequest, 2, "a non-storable 304 is retried without a cached body");
  assert.equal(requests.at(-1).options.cache, "no-store");
  assert.equal(requests.at(-1).options.headers["If-None-Match"], undefined);
  assert.ok(!cacheGraph.cache.has("visibility-race"));

  globalThis.fetch = async () =>
    graphResponse({ body: document, authoritative: "false" });
  await assert.rejects(
    cacheGraph.fetchDocumentNow("non-authoritative"),
    /non-authoritative dependency graph representation/
  );

  globalThis.fetch = async () =>
    graphResponse({ body: document, contentLength: 1 });
  await assert.rejects(
    cacheGraph.fetchDocumentNow("wrong-length"),
    /length did not match/
  );
} finally {
  globalThis.fetch = originalFetch;
}

const serialized = new ZedDependencyGraph();
const events = [];
serialized.fetchDocumentNow = async (url) => {
  events.push(`start:${url}`);
  await new Promise((resolve) => setTimeout(resolve, 2));
  events.push(`end:${url}`);
  return { url };
};
await Promise.all([
  serialized.fetchDocument("private-a", { serialized: true }),
  serialized.fetchDocument("private-b", { serialized: true }),
]);
assert.deepEqual(events, [
  "start:private-a",
  "end:private-a",
  "start:private-b",
  "end:private-b",
]);

const staleLifecycle = new ZedDependencyGraph();
staleLifecycle.fetchGeneration = 1;
await assert.rejects(
  staleLifecycle.fetchDocumentNow("stale-graph", { generation: 0 }),
  /request was cancelled/
);

for (let index = 0; index < 120; index += 1) {
  serialized.rememberDocument(`graph-${index}`, { index });
}
assert.equal(serialized.cache.size, 4, "document cache remains bounded");
assert.ok(!serialized.cache.has("graph-0"), "the oldest cached graph is evicted first");

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

const wideProjection = projectionSvgDocument({
  nodes: [...fixtures.nodes.values()],
  edges: fixtures.edges,
  positions: fixtures.positions,
  roots: fixtures.roots,
  selectedId: [...fixtures.roots][0],
  title: "Wide <projection>",
});
assert.ok(wideProjection);
assert.ok(wideProjection.width <= 4096);
assert.ok(wideProjection.height <= 4096);
assert.match(wideProjection.svg, /Wide &lt;projection&gt;/);
assert.match(wideProjection.svg, /400 relationships/);
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
