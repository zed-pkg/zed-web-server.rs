import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const css = readFileSync(new URL("../assets/dependency-graph.css", import.meta.url), "utf8");

function zIndexFor(selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const matches = [...css.matchAll(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, "g"))];
  assert.ok(matches.length, `${selector} must have an explicit declaration block`);
  const values = matches
    .map((match) => match[1].match(/(?:^|;)\s*z-index\s*:\s*(-?\d+)\s*(?:;|$)/))
    .filter(Boolean)
    .map((match) => Number(match[1]));
  assert.ok(values.length, `${selector} must define an explicit numeric z-index`);
  return values.at(-1);
}

test("toolbar popovers stack above query and graph content", () => {
  const toolbar = zIndexFor(".dg-toolbar");
  const querybar = zIndexFor(".dg-querybar");
  assert.ok(toolbar > querybar, "the Download menu must clear the later query toolbar");
  assert.ok(querybar > 1, "the Edge filters menu must clear status and graph siblings");
});

test("popover panels retain a local stacking layer", () => {
  assert.match(
    css,
    /\.dg-export-menu\s*>\s*div\s*,\s*\.dg-filter-panel\s*\{[^}]*z-index\s*:\s*30\s*;/s,
  );
});
