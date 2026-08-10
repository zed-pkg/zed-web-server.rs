# zed-web-server

The human-facing [zed-pkg](https://zpkg.net) registry UI, built on the MASH
stack: **M**aud typed HTML templates, **A**xum, **S**eaORM (never bare SQLx),
and **H**TMX for live search. Dark theme in the brand palette (black,
orange `#FF7A1A`, baby blue `#8FD3F4`).

Pages include home and recent packages, `/search` with HTMX-live results
(`/partials/search`), `/p/{org}/{name}` package pages, organization dashboards,
project and package settings, and user settings.

## Shared Auth browser ceremonies

Zed intentionally supports two distinct Shared Auth integrations:

- **PKCE/BFF handoff:** `/auth/sign-in` redirects to Shared Auth and returns to
  the exact Zed product callback `/auth/shared/callback`. The web server redeems
  the one-time code, rotates the Shared Auth refresh handle, delegates narrow
  Zed API credentials, and keeps those credentials out of the browser.
- **Same-origin proxied ceremony:** `/shared-auth-ui/*` exposes a narrow set of
  Shared Auth-owned browser pages beneath `app.zpkg.net`. This mode normally
  does not use the Zed callback.

These are first-class alternatives for different scenarios, not aliases for one
another. They use distinct route namespaces, cookies, parsers, sessions, and
tests while converging on the same canonical Shared Auth principal and Zed
account projection.

The normative product-side contract is
[`docs/shared-auth/README.md`](docs/shared-auth/README.md). It defines route
ownership, web-server versus API-server back-channel calls, cookie and logout
semantics, deployment configuration, and the dual-mode E2E matrix. The broad
`/shared-auth/*` reverse proxy remains a legacy compatibility surface; new
browser integrations use `/shared-auth-ui/*`, and confidential APIs use the
cluster-internal Shared Auth origin.

## Schema ownership

This service reads the same Postgres the API server writes. Canonical reads run
through `zed-orm-core`; canonical writes remain in `zed-api-server.rs` and its
write-enabled ORM contexts.

## Offline mode

If `DATABASE_URL` is unset or unreachable the server still boots and serves
every page with a "registry offline" banner and empty states — handy for UI
work with zero infrastructure and asserted by the test suite.

## Configuration (env)

| Var | Default |
| --- | --- |
| `BIND_ADDR` | `0.0.0.0:8081` |
| `DATABASE_URL` | unset (offline mode) |
| `PUBLIC_REGISTRY_URL` | `https://registry.zpkg.net` |
| `RUST_LOG` | `info` |
| `SHARED_AUTH_URL` | unset (Shared Auth back channel and proxies unavailable) |

`SHARED_AUTH_URL` is the cluster-internal Shared Auth origin, for example
`http://127.0.0.1:8120`. For the same-origin ceremony, Shared Auth renders links
with `AUTH_BROWSER_PUBLIC_PREFIX=/shared-auth-ui`; Zed strips that prefix before
forwarding the allow-listed browser routes. For the PKCE/BFF ceremony, the
browser-visible Shared Auth origin, exact handoff client, callback, audience,
scopes, API origin, and product-cookie signing key are configured separately as
documented in [`docs/shared-auth/README.md`](docs/shared-auth/README.md).

## Run it

```sh
# against the api server's compose postgres
DATABASE_URL=postgres://zed:zed@localhost:5432/zed cargo run

# or with no infrastructure at all (offline mode)
cargo run
```

`static/htmx.min.js` is vendored (htmx 2) so the UI has zero CDN
dependencies at runtime.

## Development

Clone side by side with `zed-interfaces` and `zed-lib-core`, then run the
repository checks:

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

## License

MIT
