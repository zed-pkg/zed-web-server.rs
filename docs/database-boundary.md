# Read-only database boundary

Tracking: [DEN-2788](https://linear.app/denman/issue/DEN-2788/zed-pkg-shared-seaorm-boundary-orm-crate-in-zed-lib-zed-api-serverzed)

`zed-web-server` is a presentation tier. It may execute approved reads for server-rendered pages, but `zed-api-server` remains the sole request-serving writer and the owner of every registry invariant.

## Defense in depth

A production web deployment must satisfy all of these controls:

1. `DATABASE_URL` resolves to a dedicated `zed_pkg__web_ro`-style principal, never the API or migration principal.
2. The role receives schema `USAGE` and an explicit `SELECT` allowlist only. It receives no `INSERT`, `UPDATE`, `DELETE`, `TRUNCATE`, `CREATE`, `ALTER`, `DROP`, ownership, or role-switch privilege.
3. Every pooled connection starts with `default_transaction_read_only=on`.
4. Startup verifies `current_setting('default_transaction_read_only') = 'on'`; a silently dropped startup option is treated as connection failure and the existing offline-mode behavior takes over.
5. Mutations go to `zed-api-server` over private-cluster HTTP with keep-alive. Retry only operations with an idempotency contract.
6. Direct reads remain bounded, tenant-scoped, and redacted. The target shared library exposes named `queries::read` functions rather than an unrestricted ORM session.

The current server implements controls 3 and 4 directly at its SeaORM/SQLx composition seam so the safety property does not wait for package rollout.

## Shared ORM package

The root `.zpkg.toml` imports `zed-pkg/zed-lib`. `zed-lib` PR #1 adds `zed-orm`, including `DbRole::ReadOnly`, `assert_read_only`, `ORG_SCHEMA = "zed_pkg"`, and a named-query split.

After that PR merges and this repository can regenerate its locked dependency graph, the intended Cargo dependency is:

```toml
zed-orm = {
  package = "zed-orm",
  git = "https://github.com/zed-pkg/zed-lib.git",
  rev = "6b7bdcc984a75997d5b72f01a17d9eca507c9a01"
}
```

Replace the reviewed branch-head revision with the merge commit before enabling it. The direct connection code in `src/server.rs` should then collapse to:

```rust
let database = zed_orm::connect(&url, zed_orm::DbRole::ReadOnly).await?;
zed_orm::assert_read_only(&database).await?;
```

Handlers may call only `zed_orm::queries::read`. The Cargo dependency is intentionally documented rather than committed before `Cargo.lock` can be regenerated; CI builds use locked resolution.

## Schema and migrations

The target namespace is `zed_pkg`. The web process does not contain or run migrations. Production DDL is generated, verified, and applied by a discrete `dpm` release job under a separate migrator credential. Schema cutover from any legacy `public` objects must use expand/backfill/contract and remain compatible with both API and web revisions during rollout.

## Offline mode

Failing the read-only assertion is equivalent to an unavailable database, not permission to widen access. The process logs the bounded startup failure and serves its established offline UI. A configuration mistake therefore degrades availability rather than creating a write-capable browser-facing process.
