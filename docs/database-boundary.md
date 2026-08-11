# Read-only database boundary

Tracking: [DEN-2788](https://linear.app/denman/issue/DEN-2788/zed-pkg-shared-seaorm-boundary-orm-crate-in-zed-lib-zed-api-serverzed)

The canonical read plane is the `rust-orm` slice of
[`zed-pkg/zed-lib-core`](https://github.com/zed-pkg/zed-lib-core). That slice
publishes the Rust crate `zed-orm-core`; this repository pins its reviewed Git
revision in both `Cargo.toml` and `Cargo.lock`.

`zed-web-server` is a presentation tier. It may execute approved reads for
server-rendered pages, but `zed-api-server` remains the sole request-serving
writer and the owner of every registry invariant.

## Defense in depth

A production web deployment must satisfy all of these controls:

1. `DATABASE_URL` resolves to a dedicated `zed_pkg__web_ro`-style principal,
   never the API or migration principal.
2. The role receives schema `USAGE` and an explicit `SELECT` allowlist only. It
   receives no `INSERT`, `UPDATE`, `DELETE`, `TRUNCATE`, `CREATE`, `ALTER`,
   `DROP`, ownership, or role-switch privilege.
3. Every pooled connection starts with `default_transaction_read_only=on`.
4. Connection setup verifies PostgreSQL accepted the read-only setting before
   returning a context to application state. A missing or ignored setting is a
   connection failure, not permission to widen access.
5. Mutations go to `zed-api-server` over its private back channel and are
   retried only when the operation has an idempotency contract.
6. Reads remain bounded, tenant-scoped, and redacted. The ORM crate exposes
   named policy-aware functions through an opaque `ReadContext`, not an
   unrestricted ORM session.

The application calls `connect_read_only_with_policy` and receives only a
`ReadContext`. Pool construction, the startup option, and the live assertion
remain inside the canonical ORM boundary, so web code cannot accidentally
recreate a weaker direct SeaORM or SQLx connection path.

## Package and feature boundary

The repository-level `.zpkg.toml` imports `zed-pkg/zed-lib-core`, matching the
Git source that supplies `zed-orm-core` to Cargo. The current immutable Cargo
pin is:

```toml
zed-orm-core = {
  git = "https://github.com/zed-pkg/zed-lib-core.git",
  rev = "c3d486a1519381276fbec02aa25247f542924443"
}
```

The dependency uses only the crate's default read-only surface. This web
process must never enable `read-write` or `migrate`, publicly expose a raw
connection, or add an independent direct SQL dependency. The separately named
`zed-orm-core` repository must not become a second source of truth without an
explicit architecture migration that updates package metadata, Cargo inputs,
provenance documentation, and the lockfile together.

## Schema and migrations

The target namespace is `zed_pkg`. The web process does not contain or run
migrations. Production DDL is generated, verified, and applied by a discrete
release job under a separate migrator credential. Schema cutover from any
legacy `public` objects must use expand/backfill/contract and remain compatible
with both API and web revisions during rollout.

## Offline mode

Failing the read-only assertion is equivalent to an unavailable database. The
process logs the bounded startup failure and serves its established offline UI.
A configuration mistake therefore degrades availability rather than creating a
write-capable browser-facing process.
