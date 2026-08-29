import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

import {
  PROFILE_MAX_EDGES,
  PROFILE_MAX_NODES,
  PROFILE_MAX_TEXT_BYTES,
} from "../static/dependency-graph-insights-core.js";

const contractUrl = new URL(
  "../static/claritas/zed-dependency-graph.component.json",
  import.meta.url
);
const provenanceUrl = new URL(
  "../static/claritas/zed-dependency-graph.provenance.json",
  import.meta.url
);
const webSourceUrl = new URL(
  "../static/dependency-graph-insights-core.js",
  import.meta.url
);

function parseJson(url) {
  return JSON.parse(readFileSync(url, "utf8"));
}

function digest(url) {
  return `sha256:${createHash("sha256").update(readFileSync(url)).digest("hex")}`;
}

function sorted(values) {
  return [...values].sort((left, right) => left.localeCompare(right));
}

const contract = parseJson(contractUrl);
const provenance = parseJson(provenanceUrl);

assert.equal(contract.schema, "claritas/component-contract/v1");
assert.equal(contract.component, "zed-dependency-graph");
assert.equal(contract.version, "1.0.0");
assert.equal(contract.producer, "claritas-viz/data-viz-server.rs");
assert.equal(contract.input.schema, "zpkg/dependency-graph/v1");
assert.equal(contract.input.authorityRequired, true);
assert.deepEqual(
  sorted(contract.targets.map(({ id }) => id)),
  ["flutter", "rust-dioxus", "rust-leptos", "web-esm"]
);
assert.equal(new Set(contract.targets.map(({ entrypoint }) => entrypoint)).size, 4);
const targets = new Map(contract.targets.map((target) => [target.id, target]));
assert.equal(targets.get("rust-leptos").manifest, "rust/Cargo.toml");
assert.equal(targets.get("rust-leptos").feature, "leptos");
assert.equal(targets.get("rust-dioxus").manifest, "rust/Cargo.toml");
assert.equal(targets.get("rust-dioxus").feature, "dioxus");
assert.equal(targets.get("flutter").manifest, "dart/pubspec.yaml");
assert.equal(contract.limits.maxNodes, PROFILE_MAX_NODES);
assert.equal(contract.limits.maxEdges, PROFILE_MAX_EDGES);
assert.equal(contract.limits.maxTextBytes, PROFILE_MAX_TEXT_BYTES);
assert.equal(contract.limits.maxRenderedNodes, 750);
assert.equal(contract.limits.maxRenderedEdges, 2500);
assert.equal(contract.limits.maxMinimapNodes, 500);
assert.equal(contract.limits.maxMinimapEdges, 900);
assert.equal(contract.trust.distribution, "vendored-source-only");
assert.equal(contract.trust.integrity, "sha256");
assert.equal(contract.trust.runtimeNetwork, false);
assert.equal(contract.trust.dynamicCodeEvaluation, false);
assert.equal(contract.trust.externalBrowserRuntime, false);

assert.equal(provenance.schema, "claritas/component-provenance/v1");
assert.equal(provenance.component, contract.component);
assert.equal(provenance.version, contract.version);
assert.match(provenance.source.revision, /^[0-9a-f]{40}$/);
assert.equal(
  provenance.source.repository,
  "https://github.com/claritas-viz/data-viz-server.rs"
);
assert.equal(provenance.integrity.algorithm, "sha256");
assert.equal(provenance.integrity.contract, digest(contractUrl));
assert.equal(provenance.integrity.webSource, digest(webSourceUrl));
assert.match(provenance.integrity.package, /^sha256:[0-9a-f]{64}$/);
assert.equal(provenance.vendored.contract, "/static/claritas/zed-dependency-graph.component.json");
assert.equal(provenance.vendored.webSource, "/static/dependency-graph-insights-core.js");
assert.equal(provenance.trust.runtimeNetwork, false);
assert.equal(provenance.trust.dynamicCodeEvaluation, false);
assert.equal(provenance.trust.externalBrowserRuntime, false);

const webSource = readFileSync(webSourceUrl, "utf8");
assert.ok(!webSource.includes("fetch("));
assert.ok(!/import\s+(?:[^;]+from\s+)?["']https?:\/\//.test(webSource));
assert.ok(!webSource.includes("eval("));
assert.ok(!webSource.includes("new Function("));

console.log("Claritas component contract and provenance checks passed");
