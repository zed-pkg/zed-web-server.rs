# Read-only database boundary

Tracking: [DEN-2788](https://linear.app/denman/issue/DEN-2788/zed-pkg-shared-seaorm-boundary-orm-crate-in-zed-lib-zed-api-serverzed)

Canonical ORM owner: [`zed-pkg/zed-orm-core`](https://github.com/zed-pkg/zed-orm-core), currently under review in [PR #1](https://github.com/zed-pkg/zed-orm-core/pull/1).

`zed-web-server` is a presentation tier. It may execute approved reads for server-rendered pages, but `zed-api-server` remains the sole request-serving writer and the owner of every registry invariant.

## Defense in depth

A production web deployment must satisfy all of these controls:

1. `DATABASE_URL` resolves to a dedicated `zed_pkg__web_ro`-style principal, never the API or migration principal.
2. The role receives schema `USAGE` and an explicit `SELECT` allowlist only. It receives no `INSERT`, `UPDATE`, `DELETE`, `TRUNCATE`, `CREATE`, `ALTER`, `DROP`, ownership, or role-switch privilege.
3. Every pooled connection starts with `default_transaction_read_only=on`.
4. Startup verifies `current_setting('default_transaction_read_only') = 'on'`; a silently dropped startup option is treated as connection failure and the existing offline-mode behavior takes over.
5. Mutations go to `zed-api-server` over private-cluster HTTP with keep-alive. Retry only operations with an idempotency contract.
6. Direct reads remain bounded, tenant-scoped, and redacted. The canonical ORM crate exposes named policy-aware read functions through an opaque context rather than an unrestricted ORM session.

The current server implements controls 3 and 4 directly at its SeaORM/SQLx composition seam so the safety property does not wait for package rollout.

## Shared ORM package

The root `.zpkg.toml` imports `zed-pkg/zed-orm-core`. The canonical crate must keep its default consumer surface read-only and must not publicly expose raw SeaORM connections, entity managers, query builders, or write types.

After `zed-orm-core` PR #1 is completed and this repository can regenerate its locked dependency graph, the intended Cargo dependency is:

```toml
zed-orm-core = {
  git = "https://github.com/zed-pkg/zed-orm-core.git",
  rev = "6ed5fc430c4769cee1d4dddf297f7cb1cd63575d",
  default-features = false,
  features = ["read-only"]
}
```

The direct connection code in `src/server.rs` should then collapse to an opaque read seam such as:

```rust
use zed_orm_core::{ReadContext, connect_read_only};

let database: ReadContext = connect_read_only(&url).await?;
```

The revision above is the current head of the canonical scaffold PR, not yet a production-ready consumer pin. Before enabling it, that PR must import the Zed entity slice from `ORESoftware/k8s-libs-and-shared-defs`, implement role-aware connection and read-only assertion logic, expose working named reads behind an opaque context, compile every write symbol only under `read-write`, and add compile-fail consumers plus live PostgreSQL/CockroachDB denial evidence. Replace the scaffold revision with the merge commit after those gates pass.

The earlier `zed-lib` ORM branch is an implementation donor only and must not become a second authoritative package. The Cargo dependency is intentionally documented rather than committed before `Cargo.lock` can be regenerated reproducibly; CI builds use locked resolution.

## Schema and migrations

The target namespace is `zed_pkg`. The web process does not contain or run migrations. Production DDL is generated, verified, and applied by a discrete `dpm` release job under a separate migrator credential. Schema cutover from any legacy `public` objects must use expand/backfill/contract and remain compatible with both API and web revisions during rollout.

## Offline mode

Failing the read-only assertion is equivalent to an unavailable database, not permission to widen access. The process logs the bounded startup failure and serves its established offline UI. A configuration mistake therefore degrades availability rather than creating a write-capable browser-facing process.
