# zed-web-server

The human-facing [zed-pkg](https://zpkg.net) registry UI, built on the MASH
stack: **M**aud typed HTML templates, **A**xum, **S**eaORM (never a separate
bare SQLx dependency), and **H**TMX for live search. Dark theme in the brand
palette (black, orange `#FF7A1A`, baby blue `#8FD3F4`).

Pages include home and recent packages, `/search` with HTMX-live results
(`/partials/search`), `/p/{org}/{name}` package pages, organization dashboards,
project and package settings, and user settings.

## Platform surfaces

The UI is mobile-first and installable: a web manifest, an offline page, and a
root-scoped service worker at `/sw.js` make the registry an app on a phone or a
desktop. The worker never caches HTML — a package page can be private, and a
shared on-device cache is the wrong place for anything whose visibility depends
on who asked. `static/app.js` is progressive enhancement only (copy buttons,
current-tab marking, worker registration); every page works with it blocked.

`/console/storage` describes the configured artifact backend by rendering
`zed-api-server`'s provider-agnostic report, so it reads the same on Cloudflare
R2, S3, Google Cloud Storage, MinIO, a directory, or process memory. No vendor
dashboard is embedded. `contracts/storage-status.v1.json` is the wire contract,
carried verbatim by the API server and the Flutter app; each side asserts its
own decoder accepts it.

Feature parity against npm, crates.io, PyPI, and RubyGems — what is shipped,
what is missing, and which repository each gap belongs to — is tracked in
[registry-ui-parity.md](docs/registry-ui-parity.md).

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

`zed-api-server` is the sole runtime writer and owns registry invariants. This
web process receives a separate SELECT-only database principal. Canonical reads
run through the default read-only `zed-orm-core` surface in `zed-lib-core`.
`connect_read_only_with_policy` starts every pooled connection with
`default_transaction_read_only=on` and verifies the setting before the
connection enters application state. A failed check falls back to the
established offline mode; it never widens access.

The exact library revision, grants, named-read-query contract, and migration
boundary are documented in
[`docs/database-boundary.md`](docs/database-boundary.md).

## Offline mode

If `DATABASE_URL` is unset, unreachable, or not actually read-only, the server
still boots and serves every page with a "registry offline" banner and empty
states — useful for UI work with zero infrastructure and safer than accepting a
write-capable browser-facing credential.

## Configuration (env)

| Var | Default |
| --- | --- |
| `BIND_ADDR` | `0.0.0.0:8081` |
| `DATABASE_URL` | unset (offline mode); must be the web SELECT-only credential |
| `DB_MAX_CONNECTIONS` | `10` |
| `DB_STATEMENT_TIMEOUT_MS` | `8000` |
| `DB_CONNECT_MAX_WAIT_SECS` | `30` |
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
# Use a dedicated read-only role; do not reuse the API/migrator URL.
DATABASE_URL=postgres://zed_web_ro:...@localhost:5432/zed cargo run

# or with no infrastructure at all (offline mode)
cargo run
```

`static/htmx.min.js` is vendored (htmx 2) so the UI has zero CDN
dependencies at runtime.

The dependency-graph Web Component and styles are reviewable in `assets/`.
Use the exact Node release in `.node-version` when running
`node scripts/build-graph-assets.mjs`; it produces the Brotli files embedded by
Axum. Because Brotli encoders can emit different valid streams across operating
systems, `--check` validates the portable invariant: each committed stream must
decode byte-for-byte to its reviewable source. Both CI and the container
publisher install the pinned JavaScript runtime. The browser never loads
Claritas or another external visualization runtime.

## Development

Cargo fetches the exact reviewed `zed-interfaces` and `zed-lib-core` revisions
recorded in the manifest and lockfile. Run the repository checks with:

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

## License

MIT
