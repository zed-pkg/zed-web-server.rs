# Immutable dependency provenance

The `app.zpkg.net` web server is compiled against reviewed, immutable source revisions rather than moving branches.

| Dependency | Revision | Role |
| --- | --- | --- |
| `zed-pkg/zed-lib-core` | `700f1f9578c6633a20693a5b1f52970ab845a740` | Canonical `zed-orm-core` read-only data plane |
| `zed-pkg/zed-interfaces` | `5394b2e7b070354ee79a4c6ac79c26d6264970cd` | Polyglot package and API contract workspace |

`Cargo.toml` and `Cargo.lock` must agree on the exact `zed-lib-core` revision. The web process uses only the default read-only ORM surface; it does not enable `read-write` or `migrate`. Mutations are sent to the API tier, and database credentials supplied to this service must map to the SELECT-only registry role.

The lock graph was regenerated and verified with Rust 1.97.1 using locked format, compile, Clippy, and test commands before this provenance record was committed. CI repeats those checks on every human-authored head.
