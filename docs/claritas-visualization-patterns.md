# Claritas visualization patterns in the Zed dependency graph

This change borrows **design principles**, not runtime code, from the public
Claritas presentation at
`claritas-viz/claritas-viz.github.io@17d1c1672f90dac600e3e292e6081a7407f1be8f`.
The Zed graph remains served entirely by `zed-web-server.rs` under the existing
same-origin CSP and does not contact a Claritas service, CDN, or external
visualization runtime.

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
- a Kahn-pass cycle ratio.

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
