# Immutable dependency provenance

The `app.zpkg.net` web server is compiled against reviewed, immutable source revisions rather than moving branches.

| Dependency | Revision | Role |
| --- | --- | --- |
| `zed-pkg/zed-lib-core` | `18fd49ba4d73689a941eaa56ef20a12a6c90a993` | Canonical `zed-orm-core` read-only data plane |
| `zed-pkg/zed-interfaces` | `15577e17a820c3b2b1a39ee178d4645185309a05` | Polyglot package and API contract workspace |

`Cargo.toml` and `Cargo.lock` must agree on both exact revisions. The web process uses only the default read-only ORM surface; it does not enable `read-write` or `migrate`. Mutations are sent to the API tier, and database credentials supplied to this service must map to the SELECT-only registry role.

The lock graph was regenerated and verified with Rust 1.97.1 using locked format, compile, Clippy, and test commands before this provenance record was committed. CI repeats those checks on every human-authored head.
