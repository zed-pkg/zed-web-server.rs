import assert from "node:assert/strict";
import { test } from "node:test";

import {
  ZedDependencyGraph,
  parseGraphViewState,
} from "../assets/dependency-graph.js";

const KINDS = ["runtime", "build", "development", "peer", "tooling"];

// Exercise the actual shell renderer without a browser or a replacement
// template. Query results are irrelevant until controls are bound.
function graphForShell(mode = "package") {
  const graph = new ZedDependencyGraph();
  graph.dataset = {
    mode,
    version: "1.0.0",
    versions: JSON.stringify([{ version: "1.0.0" }]),
  };
  graph.mode = mode;
  graph.versions = [{ version: "1.0.0" }];
  graph.defaultVersion = "1.0.0";
  graph.querySelector = () => null;
  graph.querySelectorAll = () => [];
  graph.setAttribute = () => {};
  return graph;
}

function checkedInput(markup, attribute, value) {
  const inputs = markup.match(/<input\b[^>]*>/g) || [];
  const matches = inputs.filter((input) => input.includes(`${attribute}="${value}"`));
  assert.equal(matches.length, 1, `exactly one ${attribute}=${value} control`);
  return /\schecked(?:\s|=|\/?>)/.test(matches[0]);
}

function assertFilterControls(graph, state) {
  assert.equal(
    checkedInput(graph.innerHTML, "data-control", "optional"),
    state.includeOptional,
    "optional control must match the parsed model before graph data loads",
  );
  for (const kind of KINDS) {
    assert.equal(
      checkedInput(graph.innerHTML, "data-kind", kind),
      state.kinds.includes(kind),
      `${kind} control must match the parsed model before graph data loads`,
    );
  }
}

for (const mode of ["package", "scope"]) {
  for (let mask = 0; mask < 1 << KINDS.length; mask += 1) {
    for (const includeOptional of [false, true]) {
      const kinds = KINDS.filter((_, index) => mask & (1 << index));
      test(`${mode}: kind mask ${mask}, optional ${includeOptional}`, () => {
        const url = new URL("https://zpkg.invalid/p/example/package");
        url.searchParams.set("graph-kinds", kinds.join(","));
        url.searchParams.set("graph-optional", includeOptional ? "1" : "0");
        const state = parseGraphViewState(url.href);
        assert.deepEqual(state.kinds, kinds);
        assert.equal(state.includeOptional, includeOptional);
        const graph = graphForShell(mode);
        graph.applyParsedViewState(state);
        graph.renderShell();
        assertFilterControls(graph, state);
        // A subsequent shell rebuild must preserve the same selection.
        graph.renderShell();
        assertFilterControls(graph, state);
      });
    }
  }
}

test("absent filter parameters retain all-kind and optional defaults", () => {
  const state = parseGraphViewState("https://zpkg.invalid/p/example/package");
  assert.deepEqual(state.kinds, KINDS);
  assert.equal(state.includeOptional, true);
  const graph = graphForShell();
  graph.applyParsedViewState(state);
  graph.renderShell();
  assertFilterControls(graph, state);
});

test("unknown kinds cannot check controls omitted by the URL", () => {
  const state = parseGraphViewState(
    "https://zpkg.invalid/p/example/package?graph-kinds=build,unknown,build&graph-optional=0",
  );
  assert.deepEqual(state.kinds, ["build"]);
  const graph = graphForShell();
  graph.applyParsedViewState(state);
  graph.renderShell();
  assertFilterControls(graph, state);
});

test("initial connection exposes restored filters before binding or loading", () => {
  const originalLocation = Object.getOwnPropertyDescriptor(globalThis, "location");
  const url = "https://zpkg.invalid/p/example/package?graph-kinds=peer,tooling&graph-optional=0";
  Object.defineProperty(globalThis, "location", {
    configurable: true,
    value: new URL(url),
  });
  try {
    const state = parseGraphViewState(url);
    const graph = graphForShell();
    const calls = [];
    graph.bindControls = () => {
      assertFilterControls(graph, state);
      calls.push("bind");
    };
    graph.loadInitial = () => {
      assertFilterControls(graph, state);
      calls.push("load");
    };
    graph.connectedCallback();
    assert.deepEqual(calls, ["bind", "load"]);
    assert.equal(graph.includeOptional, false);
    assert.deepEqual([...graph.enabledKinds], ["peer", "tooling"]);
  } finally {
    if (originalLocation) Object.defineProperty(globalThis, "location", originalLocation);
    else delete globalThis.location;
  }
});
