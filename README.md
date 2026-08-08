# zed-web-server

The human-facing [zed-pkg](https://zpkg.net) registry UI, built on the MASH
stack: **M**aud typed HTML templates, **A**xum, **S**eaORM (never bare SQLx),
and **H**TMX for live search. Dark theme in the brand palette (black,
orange `#FF7A1A`, baby blue `#8FD3F4`).

Pages: home (recent packages), `/search` with HTMX-live results
(`/partials/search`), `/p/{org}/{name}` package pages with the provenance
column (VCS tag per version, sha256, sizes), and `/orgs/{org}`.

## Schema ownership

This service reads the same Postgres the API server writes. The schema's
source of truth is `zed-api-server.rs/migration/`; the read-only entities in
`src/entities/` mirror it and must be kept in sync.

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

Clone side by side with `zed-interfaces` (path dependency), then `cargo test`.

## License

MIT
