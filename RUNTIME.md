# Registry web runtime boundary

`zed-web-server` is a read-only MASH application. The API server remains the
schema and write-path authority.

## Module ownership

- `src/main.rs` is only the Tokio executable adapter.
- `src/server.rs` owns process startup, tracing, bounded initial database retry,
  offline-mode selection, state composition, listener binding, and serving.
- `src/routes.rs` owns the HTTP route catalog and middleware composition.
- `src/state.rs` owns the read-only application state.
- `src/entities.rs` owns SeaORM entity mappings used for registry reads.
- `src/views.rs` owns Maud rendering.
- `src/proxy.rs` owns the `/shared-auth` reverse-proxy handler (gateway to the
  shared-auth service; enabled by `SHARED_AUTH_URL`, otherwise 503).

The process runtime must not absorb route handlers, entity definitions, HTML
rendering, registry write behavior, migrations, or schema ownership.

## Database policy

The UI preserves its existing fail-open startup behavior:

- no `DATABASE_URL` starts immediately in offline mode;
- a configured database receives a bounded initial retry window;
- exhaustion of that window logs a warning and enters offline mode;
- the API server remains responsible for migrations and all writes.

`DB_MAX_CONNECTIONS`, `DB_STATEMENT_TIMEOUT_MS`, and
`DB_CONNECT_MAX_WAIT_SECS` retain their existing defaults of 10, 8000, and 30.
The server-side statement timeout remains below the HTTP timeout so abandoned
queries cannot accumulate after a request future is dropped.

## Regression gate

CI requires a four-line executable, a library/runtime boundary, frozen Cargo
resolution, strict Clippy, all tests, Rustdoc with warnings denied, a release
build, and source checks preventing database, router, or listener ownership from
returning to `main.rs`.
