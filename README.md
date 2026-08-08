# zed-web-server

The human-facing [zed-pkg](https://zpkg.tech) registry UI, built on the MASH
stack: **M**aud typed HTML templates, **A**xum, **S**eaORM (never a separate
bare SQLx dependency), and **H**TMX for live search. Dark theme in the brand
palette (black, orange `#FF7A1A`, baby blue `#8FD3F4`).

Pages: home (recent packages), `/search` with HTMX-live results
(`/partials/search`), `/p/{org}/{name}` package pages with the provenance
column (VCS tag per version, sha256, sizes), and `/orgs/{org}`.

## Schema ownership

`zed-api-server` is the sole runtime writer and owns registry invariants. This
web process receives a separate SELECT-only database principal. Every pooled
connection also starts with `default_transaction_read_only=on`, and startup
verifies that PostgreSQL accepted the setting before the connection enters
application state. A failed check falls back to the established offline mode;
it never widens access.

The shared-library rollout, exact `zed-orm` revision, grants, named-read-query
contract, and migration boundary are documented in
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
| `PUBLIC_REGISTRY_URL` | `https://registry.zpkg.tech` |
| `RUST_LOG` | `info` |

## Run it

```sh
# Use a dedicated read-only role; do not reuse the API/migrator URL.
DATABASE_URL=postgres://zed_web_ro:...@localhost:5432/zed cargo run

# or with no infrastructure at all (offline mode)
cargo run
```

`static/htmx.min.js` is vendored (htmx 2) so the UI has zero CDN
dependencies at runtime.

## Development

Clone side by side with `zed-interfaces` (path dependency), then run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## License

MIT
