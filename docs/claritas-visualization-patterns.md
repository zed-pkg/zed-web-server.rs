# Claritas visualization patterns in the Zed dependency graph

This change consumes the versioned `zed-dependency-graph@1.0.0` component
contract and framework-neutral topology core produced by
`claritas-viz/data-viz-server.rs@3b52ee4ea86cfb28b9e757a736671de9841b8eae`.
Claritas also publishes Rust/Leptos, Rust/Dioxus, and Dart/Flutter adapters for
the same `zpkg/dependency-graph/v1` model.

The contract and web source are copied into this repository and pinned by
SHA-256 in `static/claritas/zed-dependency-graph.provenance.json`. CI verifies
the vendored bytes, limits, targets, and trust declarations. This is a
build-time source relationship, not a browser runtime dependency: the Zed graph
remains served entirely by `zed-web-server.rs` under its same-origin CSP and
does not contact GitHub, a Claritas service, CDN, or external visualization
runtime.

## Adopted principles

### Score visual choices instead of hard-coding one view

Claritas describes visualization search as evaluating candidate specifications.
The Zed component now derives a bounded topology profile and scores its existing
layered, radial, and force layouts. The recommendation is explanatory and
user-controlled: it never silently replaces a saved or explicitly selected
layout.

The profile is deterministic and uses only the already-authorized in-browser
model:

- node and relationship counts;
- root and connected-component counts;
- maximum dependency depth and layer width;
- graph density and maximum-degree hub ratio; and
- exact iterative strongly connected components and cyclic-node ratio.

Depth is the longest path through the acyclic component graph, not shortest
BFS distance. Duplicate relationships are ignored for topology scoring, and
inputs fail closed above the shared 3,000-node/12,000-edge contract.

Force layout is never recommended above the product's existing 260-node force
budget.

### Keep an editorial summary beside the analytical canvas

The compact visual-search card explains the recommended layout and exposes the
three suitability scores. It follows the Claritas site's editorial hierarchy:
a short eyebrow, a strong conclusion, supporting rationale, and compact metrics.

### Preserve context while navigating detail

The overview navigator renders a bounded projection of the packages currently
on the main canvas and an explicit viewport rectangle. Clicking the overview
recenters the existing SVG transform; the minimap does not own graph state or
perform another data fetch.

### Degrade explicitly

The minimap caps its projection at 500 nodes and 900 relationships, prioritizing
selection, roots, and high-degree packages. This complements—not replaces—the
base component's 750-node/2,500-edge main-canvas degradation contract. Queries,
exports, and accessible data continue to use the full loaded semantic model.

## Boundaries retained

- No external scripts, styles, fonts, images, or network calls.
- No changes to graph authorization, BFF routes, canonical representations, or
  PostgreSQL reads.
- No automatic layout changes.
- No React or separately hosted Claritas runtime.
- The same package, project, and organization custom element is enhanced.
- Reduced-motion preferences disable score-bar transitions.

## Updating the vendored component

1. Review and merge the corresponding Claritas component-bundle change.
2. Copy the contract and framework-neutral web source from one immutable
   Claritas commit.
3. Update the revision and all SHA-256 values in the provenance document.
4. Run `node scripts/check-claritas-component-bundle.mjs` and the dependency
   graph test suite before opening the Zed change.

The server advertises the same-origin contract with a non-executing
`rel="alternate"` link. Consumers must never turn the bundle endpoint into a
browser-side code loader.
