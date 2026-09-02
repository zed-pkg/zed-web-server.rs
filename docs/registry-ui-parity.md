# Registry UI parity

What npmjs.com, crates.io, PyPI, and RubyGems all offer, what zed-pkg offers
today, and where the missing work lives. The point of writing it down is that
"reach parity with npm" is not a task anyone can pick up, while "the package
page has no README because `zed-orm-core` exposes no readme column" is.

Three surfaces render this UI and are expected to stay in step: the web app
(`zed-web-server.rs`), the Flutter app (`zed-app`), and the CLI's own output.
A row is only "done" when it is answerable from the API — the UIs are
projections, and a gap in the data plane is a gap everywhere.

## Legend

- **shipped** — available now.
- **partial** — present but thinner than the reference registries.
- **blocked** — the UI is straightforward; the data does not exist yet.

`owner` names the repository the next piece of work belongs in.

## Package page

| Feature | npm | crates.io | PyPI | RubyGems | zed-pkg | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| Install command, copyable | ✓ | ✓ | ✓ | ✓ | **shipped** | — |
| Version-pinned install command | ✓ | ✓ | ✓ | ✓ | **shipped** (app), partial (web) | zed-web-server |
| Description / summary | ✓ | ✓ | ✓ | ✓ | **shipped** | — |
| Rendered README | ✓ | ✓ | ✓ | ✓ | **blocked** — no readme in the read model | zed-lib-core, then both UIs |
| Version list with dates | ✓ | ✓ | ✓ | ✓ | **shipped** | — |
| Per-version page | ✓ | ✓ | ✓ | ✓ | **blocked** — no route | zed-web-server |
| Declared dependencies | ✓ | ✓ | ✓ | ✓ | **partial** — in the graph, not as a list | zed-web-server |
| Reverse dependencies / dependents | ✓ | ✓ | — | ✓ | **blocked** — needs a reverse index | zed-lib-core |
| Dependency graph visualization | — | — | — | — | **shipped** — and nobody else has it | — |
| Download counts | ✓ | ✓ | ✓ | ✓ | **partial** — a total, no time series | zed-lib-core |
| Downloads over time chart | ✓ | ✓ | — | — | **blocked** — needs the series above | zed-web-server |
| Keywords / categories, linked | ✓ | ✓ | ✓ | — | **blocked** — tags exist in search, unlinked | zed-web-server |
| License | ✓ | ✓ | ✓ | ✓ | **shipped** | — |
| Repository link | ✓ | ✓ | ✓ | ✓ | **shipped** | — |
| Homepage / docs / issues / funding links | ✓ | ✓ | ✓ | ✓ | **blocked** — manifest has no link set | zed-interfaces |
| Owners / maintainers | ✓ | ✓ | ✓ | ✓ | **blocked** — not projected to the page | zed-lib-core |
| Artifact size and digest | ✓ | — | ✓ | — | **shipped** — every version row | — |
| File listing / code browser | ✓ | — | ✓ | — | **partial** — `/v1/files` serves it, no UI | zed-web-server |
| Yank / deprecation banner | ✓ | ✓ | ✓ | ✓ | **partial** — struck through in the table only | zed-web-server |
| Provenance / attestation badge | ✓ | — | ✓ | — | **partial** — VCS tag shown; not explained | zed-web-server |
| Security advisories | ✓ | ✓ | — | — | **blocked** — no advisory data plane | zed-api-server |
| Related / similar packages | ✓ | — | — | — | **blocked** — semantic search exists, unused here | zed-web-server |

## Search and discovery

| Feature | Reference | zed-pkg | Owner |
| --- | --- | --- | --- |
| Live search | npm, crates.io | **shipped** — HTMX, 300 ms debounce | — |
| Semantic search | none of them | **partial** — `/v1/search/semantic` exists, no UI | zed-web-server |
| Result ranking signals | npm (quality/popularity/maintenance) | **blocked** | zed-api-server |
| Filter by keyword / category | crates.io, PyPI | **partial** — API takes tags, UI does not offer them | zed-web-server |
| Filter by language / ecosystem | none of them | **blocked** — zed knows this and should use it | zed-web-server |
| Sort (relevance, downloads, recent) | all four | **blocked** | zed-api-server |
| Recently updated / new | crates.io, PyPI | **shipped** — the home page | — |
| Most downloaded | all four | **blocked** — needs the counters above | zed-lib-core |

## Accounts and publishing

| Feature | Reference | zed-pkg | Owner |
| --- | --- | --- | --- |
| Sign in | all four | **shipped** — Shared Auth, PKCE/BFF | — |
| User profile page with their packages | all four | **blocked** | zed-web-server |
| Org / team pages | npm, RubyGems | **shipped** — dashboards | — |
| Role management | npm | **shipped** — org and project roles | — |
| API token management | all four | **partial** — CLI only, no web UI | zed-web-server |
| Audit log | none of them | **shipped** — and hash-chain verifiable | — |
| Publish from the browser | none of them | intentionally absent — publishing is a VCS-tag ceremony | — |

## Platform

| Feature | Reference | zed-pkg | Owner |
| --- | --- | --- | --- |
| Responsive layout | all four | **shipped** | — |
| Installable app (PWA) | none of them | **shipped** | — |
| Native mobile + desktop app | none of them | **shipped** — `zed-app` | — |
| Dark theme | crates.io, PyPI | **shipped** — the only theme | — |
| Light theme | all four | **blocked** — deliberate, revisit if asked | zed-web-server |
| Storage / artifact console | none of them | **shipped** — provider-agnostic | — |
| Offline page | none of them | **shipped** | — |
| RSS / activity feed | crates.io, RubyGems | **blocked** | zed-web-server |
| Keyboard shortcuts | npm, crates.io | **blocked** | zed-web-server |

## What the reference registries do not have

Worth stating, because parity is a floor and not a ceiling. Four of these are
already shipped and are the reasons to use zed-pkg rather than reasons it is
behind:

- **A dependency graph you can query and export**, per package, project, and
  org, in several formats.
- **Polyglot packages.** One coordinate can carry a Node, Python, Go, and Rust
  slice, and the consumer's ecosystem selects the right one. No single-language
  registry has a concept for this, and the UI barely surfaces it yet — the
  largest *unclaimed* differentiator in this document.
- **A verifiable audit log** of every change to published state.
- **A storage console that is not a vendor's dashboard.**
- **A local project registry**, so a checkout on this machine resolves without
  the network at all.

## Suggested order

Ordered by what unblocks the most, not by difficulty:

1. **README storage and rendering.** The single largest gap against all four
   reference registries, and the first thing a visitor looks for.
   (`zed-lib-core` → both UIs.)
2. **Download counters over time.** Blocks three rows on its own, and is what
   makes a package page feel alive.
3. **Dependencies and dependents as lists.** The graph is better, but a list is
   what people scan, and dependents need a reverse index either way.
4. **Per-version pages.** Cheap, and every reference registry has them.
5. **Polyglot targets in the UI.** Nobody else can do this; today it is
   invisible.
6. **Keywords, filters, and sorting.** Mostly UI over API surface that already
   exists.

## Keeping this file honest

Any change that closes a row edits the row in the same commit. A row that says
**blocked** and names an owner is a working ticket; one that quietly became
true and was never updated makes the whole table untrustworthy.
