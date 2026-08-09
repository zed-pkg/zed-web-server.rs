# Registry web runtime boundary

`zed-web-server` is the first-party MASH application for both public package
pages and the authenticated account console. `zed-lib` owns the canonical
SeaORM entities, named operations, schema, and migration authority. The API
server owns registry writes; Shared Auth owns authentication ceremonies and
revocable sessions.

## Module ownership

- `src/main.rs` is only the Tokio executable adapter.
- `src/server.rs` owns process startup, tracing, bounded initial database retry,
  read-only verification, offline-mode selection, state composition, listener
  binding, and serving.
- `src/routes.rs` owns public registry routes and middleware composition.
- `src/account.rs` owns authenticated home/dashboard/settings routes, API
  mediation, form origin enforcement, and Maud account rendering.
- `src/state.rs` owns the read-only database handle plus the internal API and
  Shared Auth routing configuration.
- `src/entities.rs` remains a compatibility mapping for public registry reads;
  new shared entity/query work belongs in `zed-lib`.
- `src/views.rs` owns public Maud rendering.
- `src/proxy.rs` owns the `/shared-auth` reverse-proxy handler, enabled by
  `SHARED_AUTH_URL` and otherwise fail-closed with 503.

The process runtime must not absorb raw registry write behavior, migration
execution, Supabase service credentials, or product authorization decisions.

## Database policy

The browser-facing process is read-only at both startup and transaction levels:

- no `DATABASE_URL` starts immediately in public offline mode;
- a configured database receives a bounded initial retry window;
- every connection requests `default_transaction_read_only=on`;
- `zed_orm::assert_read_only` verifies the setting before the pool enters state;
- a failed read-only assertion is treated as database unavailability, never as a
  reason to widen privilege;
- the API server and discrete migration job use separate write/migration
  principals.

`DB_MAX_CONNECTIONS`, `DB_STATEMENT_TIMEOUT_MS`, and
`DB_CONNECT_MAX_WAIT_SECS` retain defaults of 10, 8000, and 30. The database
statement timeout remains below the HTTP timeout so abandoned queries cannot
accumulate after a request future is dropped.

## Account API boundary

`ZED_API_URL` points at the internal registry API. The web process forwards the
verified first-party access-token cookie as a bearer token; it does not forward
arbitrary browser authentication headers. Cookie-authenticated form mutations
also require an exact `Origin` match against `PUBLIC_WEB_ORIGIN`.

`AUTH_SESSION_COOKIE_NAME` defaults to `__Host-ore-session`. It carries a
Shared Auth access token, not a registry session row. The API re-verifies the
raw token with Shared Auth for each account request, including session
revocation, and maps the immutable subject into the registry `users` entity.

## Shared Auth and Supabase boundary

`SHARED_AUTH_URL` makes the process the first-party gateway for Shared Auth.
`/shared-auth` and everything below it are forwarded with the prefix stripped;
method, query, body, and multi-value headers are preserved; hop-by-hop headers
are dropped; X-Forwarded-For/Proto/Host are supplied. Redirects and response
bodies pass through untouched so Shared Auth owns its security and ceremony
contract.

Supabase built-in auth is an identity provider behind Shared Auth. Supabase
service-role credentials and customer-auth RDS credentials must never enter the
web or registry API pods. Shared Auth translates provider identity into its
canonical subject and revocable session; the registry remains responsible for
organization, project, package, and invitation authorization.

## Regression gate

CI requires a thin executable, frozen immutable Git dependencies, strict
Clippy, all tests, Rustdoc with warnings denied, a release build, read-only pool
evidence, the complete account route catalog, static console assets, and source
checks preventing database, router, or listener ownership from returning to
`main.rs`.
