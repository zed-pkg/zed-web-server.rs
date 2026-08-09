# Immutable dependency provenance

The `app.zpkg.net` web server is compiled against reviewed, immutable source revisions rather than moving branches.

| Dependency | Revision | Role |
| --- | --- | --- |
| `zed-pkg/zed-lib-core` | `d9a1f72baad87a0bbe256ad892d61d7a4fdd9135` | Canonical `zed-orm-core` read-only data plane |
| `zed-pkg/zed-interfaces` | `07d01604461d00e237c7d86ad3855464167574ec` | Shared package and API contract types |

`Cargo.toml` and `Cargo.lock` must agree on the exact `zed-lib-core` revision. The web process uses only the default read-only ORM surface; it does not enable `read-write` or `migrate`. Mutations are sent to the API tier, and database credentials supplied to this service must map to the SELECT-only registry role.

The lock graph was regenerated and verified with Rust 1.97.1 using locked format, compile, Clippy, and test commands before this provenance record was committed. CI repeats those checks on every human-authored head.
