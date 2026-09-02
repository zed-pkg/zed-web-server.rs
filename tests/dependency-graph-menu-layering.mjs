import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const baseCss = readFileSync(
  new URL("../assets/dependency-graph.css", import.meta.url),
  "utf8",
);
const integrationCss = readFileSync(
  new URL("../static/dependency-graph-insights.css", import.meta.url),
  "utf8",
);

function numericPropertyFor(css, selector, property) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const matches = [...css.matchAll(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, "g"))];
  assert.ok(matches.length, `${selector} must have an explicit declaration block`);
  const values = matches
    .map((match) =>
      match[1].match(
        new RegExp(`(?:^|;)\\s*${property}\\s*:\\s*(-?\\d+)\\s*(?:;|$)`),
      ),
    )
    .filter(Boolean)
    .map((match) => Number(match[1]));
  assert.ok(values.length, `${selector} must define an explicit numeric ${property}`);
  return values.at(-1);
}

test("popover parents stack above later graph siblings", () => {
  const toolbar = numericPropertyFor(baseCss, ".dg-toolbar", "z-index");
  const querybar = numericPropertyFor(baseCss, ".dg-querybar", "z-index");
  const stage = numericPropertyFor(baseCss, ".dg-stage", "z-index");
  assert.ok(toolbar > querybar, "the Download menu must clear the later query toolbar");
  assert.ok(querybar > stage, "the Edge filters menu must clear status and graph content");
  assert.match(
    baseCss,
    /\.dg-export-menu\s*>\s*div\s*,\s*\.dg-filter-panel\s*\{[^}]*z-index\s*:\s*30\s*;/s,
  );
});

test("wrapped edge filters stay inside the intentionally clipped graph shell", () => {
  assert.match(baseCss, /\.dg-shell\s*\{[^}]*overflow\s*:\s*hidden\s*;/s);
  const override = integrationCss.match(/\.dg-filter-panel\s*\{([^}]*)\}/s);
  assert.ok(override, "the consumer integration must define filter-panel geometry");
  assert.match(override[1], /(?:^|;)\s*left\s*:\s*0\s*;/s);
  assert.match(override[1], /(?:^|;)\s*right\s*:\s*auto\s*;/s);
  assert.match(override[1], /min-width\s*:\s*min\(230px,\s*calc\(100vw\s*-\s*32px\)\)\s*;/s);
  assert.match(override[1], /max-width\s*:\s*calc\(100vw\s*-\s*32px\)\s*;/s);
  assert.doesNotMatch(
    integrationCss,
    /\.dg-export-menu\s*>\s*div\s*\{[^}]*left\s*:\s*0/s,
    "the right-aligned Download menu must not be moved by the filter repair",
  );
});
