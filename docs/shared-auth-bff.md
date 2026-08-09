# Shared Auth PKCE/BFF contract for app.zpkg.net

`app.zpkg.net` is a backend-for-frontend, not a browser API client. The browser
never receives a zed-pkg registry bearer token and never posts mutations across
origins.

## Login

1. `GET /auth/sign-in` creates an exact `state` value and PKCE verifier.
2. The web backend stores that transient state in an HttpOnly, host-only,
   SameSite=Lax signed cookie and redirects to Shared Auth `/authorize`.
3. Shared Auth authenticates against the Supabase project assigned to the
   `zpkg` browser-handoff client and returns one short-lived one-time code to
   `GET /auth/shared/callback`.
4. The web backend validates state and redeems the code with its backend-only
   handoff client secret and PKCE verifier.
5. It exchanges the Supabase access token for a Shared Auth RDS-backed base
   session, delegates an audience-bound `zpkg:account` token to `azp=zpkg-web`,
   and calls `/api/v1/account/me` to create/update the local registry user
   projection.
6. The browser receives only a host-only HttpOnly product cookie containing the
   opaque rotating Shared Auth refresh handle and canonical Shared Auth user id,
   protected against tampering by a dedicated HMAC key.

## Mutations

Existing Maud forms keep their stable `/v1/*` action paths, but
`PUBLIC_REGISTRY_URL` is no longer used for browser writes. The web server
terminates those same-origin form posts, checks `Origin` (or a same-origin
`Referer` fallback), rotates the Shared Auth refresh token, delegates a fresh
short-lived token for `aud=zed-pkg`, `azp=zpkg-web`, `scope=zpkg:account`, and
forwards JSON to the canonical `/api/v1/account/*` route.

The registry Postgres identity used by this web process remains read-only. All
writes still pass through the API's SeaORM write context and database policy
triggers, including the 10-day/50-download private-to-public invariant.

## Required configuration

| Variable | Purpose |
| --- | --- |
| `PUBLIC_BASE_URL` | exact app origin, e.g. `https://app.zpkg.net` |
| `SHARED_AUTH_URL` | cluster-internal Shared Auth origin |
| `SHARED_AUTH_PUBLIC_URL` | browser-visible Shared Auth origin |
| `SHARED_AUTH_HANDOFF_CLIENT_ID` | exact handoff client, default `zpkg` |
| `SHARED_AUTH_HANDOFF_CLIENT_SECRET` | backend-only handoff redemption secret |
| `SHARED_AUTH_DELEGATE_CLIENT_ID` | delegated-token authorized party, default `zpkg-web` |
| `SHARED_AUTH_AUDIENCE` | delegated audience, default `zed-pkg` |
| `SHARED_AUTH_SCOPES` | comma/space-delimited scopes; must include `zpkg:account` |
| `ZED_API_URL` | cluster-internal API origin |
| `ZED_SESSION_SIGNING_SECRET` | dedicated cookie HMAC key, at least 32 bytes |

Production TLS uses `__Host-` cookie names. No Supabase service-role key, Shared
Auth service credential, registry API bearer token, or R2 secret belongs in the
browser or this repository.
