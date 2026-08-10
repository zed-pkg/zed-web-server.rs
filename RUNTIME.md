# Registry web runtime boundary

`zed-web-server` is the read-only MASH registry and the first-party
`app.zpkg.net` backend-for-frontend. The API server remains the schema and
write-path authority; Shared Auth owns authentication ceremonies and revocable
sessions.

## Module ownership

- `src/main.rs` is only the Tokio executable adapter.
- `src/server.rs` owns configuration, bounded database retry, read-only state
  composition, listener binding, and serving.
- `src/routes/` owns the public registry pages, account console pages, stable
  form routes, and middleware composition.
- `src/browser_auth.rs` owns exact-client PKCE handoff, session rotation,
  same-origin mutation mediation, and delegated API calls.
- `src/session.rs` resolves the viewer without exposing session credentials to
  templates.
- `src/state.rs` contains the read-only data context and secret-bearing BFF
  configuration; those secret fields deliberately do not implement `Debug`.
- `src/views/` owns reusable Maud layout and components.
- `src/proxy.rs` owns the bounded `/shared-auth` gateway and preserves stricter
  upstream security headers.

The process must not absorb raw registry writes, migrations, Supabase service
credentials, or product authorization decisions.

## Database policy

The browser-facing process remains read-only at both type and connection
levels:

- no `DATABASE_URL` starts in public offline mode;
- a configured database receives a bounded initial retry window;
- `zed_orm_core::connect_read_only_with_policy` establishes and certifies the
  SELECT-only context before it enters application state;
- exhaustion logs a warning and serves public offline pages;
- every account mutation travels through the API's write context.

`DB_MAX_CONNECTIONS`, `DB_STATEMENT_TIMEOUT_MS`, and
`DB_CONNECT_MAX_WAIT_SECS` default to 10, 8000, and 30. The database statement
timeout stays below the HTTP timeout so abandoned queries cannot accumulate.

## Shared Auth PKCE/BFF boundary

`ZED_API_URL` identifies the internal registry API. The browser receives only
signed, HttpOnly, host-only login and product-session cookies; it never receives
the delegated registry bearer token. Each mutation requires an exact same-origin
`Origin` or `Referer`, rotates the Shared Auth session, delegates a fresh
audience- and scope-bound token, and calls the canonical `/api/v1/account/*`
API.

`SHARED_AUTH_URL` is the cluster-internal Auth origin and
`SHARED_AUTH_PUBLIC_URL` is the browser-visible origin. `/shared-auth` forwards
method, query, body, and end-to-end headers while dropping hop-by-hop headers.
Redirects and response bodies pass through unchanged, and the proxy never
replaces a stricter upstream security policy.

The complete login, cookie, mutation, and failure contract is specified in
`docs/shared-auth-pkce-bff.md`.

## Regression gate

CI requires the pinned sibling interface workspace, a frozen lockfile, a thin
executable, strict formatting and Clippy, all tests, warning-free Rustdoc, a
release build, and source checks that keep database, router, and listener
ownership outside `main.rs`.
