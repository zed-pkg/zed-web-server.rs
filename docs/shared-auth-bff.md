# Shared Auth PKCE/BFF contract for app.zpkg.net

`app.zpkg.net` is a backend-for-frontend, not a browser API client. The browser
never receives a Zpkg registry bearer token and never posts mutations across
origins.

## Domain and session decision

Shared Auth may—and for the canonical multi-product deployment should—use a
public domain different from Zpkg:

```text
https://app.zpkg.net
  -> https://auth.oresoftware.dev/authorize
  -> https://app.zpkg.net/auth/shared/callback?code=...&state=...
```

The exact callback is:

```text
https://app.zpkg.net/auth/shared/callback
```

It is a **Shared Auth -> Zpkg** callback, not a direct Supabase callback.
Supabase/provider callbacks, provider-token verification, MFA, and assurance
remain inside Shared Auth. The Zpkg callback receives only a short-lived,
one-time authorization code and the exact `state` value.

Each origin owns a separate host-only cookie:

```text
auth.oresoftware.dev
  __Host-shared-auth-session   # central Shared Auth/SSO session

app.zpkg.net
  __Host-zpkg-session          # Zpkg BFF/product session
```

Never set `Domain=.zpkg.net` or another parent-domain cookie. Production cookies
use the `__Host-` prefix, `Secure`, `HttpOnly`, `Path=/`, and no `Domain`
attribute. A user can still receive SSO because the browser sends the central
cookie when redirected back to the Shared Auth origin; cookie sharing is not
required.

An optional `https://auth.zpkg.net` vanity hostname may route to the same Shared
Auth deployment for branding. It is an alias/front door, not a second auth
implementation and not a mechanism for sharing cookies with `app.zpkg.net`.

The existing same-origin `/shared-auth/*` reverse proxy may remain for selected
account-security pages, migration compatibility, or emergency fallback. It is
not the canonical Zpkg login/session path after this PKCE/BFF flow is enabled
and must not create a second competing Zpkg session model.

## Login

1. `GET /auth/sign-in` creates an exact `state` value and PKCE verifier.
2. The web backend stores the transient state, verifier, and validated local
   return path in an HttpOnly, host-only, SameSite=Lax signed cookie.
3. It redirects the browser to `SHARED_AUTH_PUBLIC_URL/authorize` with the exact
   `client_id=zpkg`, exact callback URI, state, and PKCE S256 challenge.
4. Shared Auth authenticates against the Supabase project assigned to the
   `zpkg` browser-handoff client and returns one short-lived one-time code to
   `GET /auth/shared/callback`.
5. The web backend validates state and redeems the code with its backend-only
   handoff client secret and PKCE verifier by calling the cluster-internal
   `SHARED_AUTH_URL`.
6. It exchanges the verified Supabase access token for a Shared Auth RDS-backed
   base session, delegates an audience-bound `zpkg:account` token to
   `azp=zpkg-web`, and calls `/api/v1/account/me` to create or update the local
   registry user projection.
7. The browser receives only a host-only HttpOnly Zpkg product cookie containing
   the opaque rotating Shared Auth refresh handle and canonical Shared Auth user
   ID, protected against tampering by a dedicated HMAC key. The transient login
   cookie is cleared before redirecting to the validated local page.

Browser-visible and back-channel addresses are deliberately separate:

```text
SHARED_AUTH_PUBLIC_URL=https://auth.oresoftware.dev
SHARED_AUTH_URL=http://shared-auth-server.shared-auth.svc.cluster.local
```

Authorization redirects use the public URL. Handoff redemption, exchange,
refresh, delegation, logout, and introspection use the internal URL when the
services share a trusted cluster/network.

## Mutations

Existing Maud forms keep their stable `/v1/*` action paths, but
`PUBLIC_REGISTRY_URL` is no longer used for browser writes. The web server
terminates those same-origin form posts, checks `Origin` (or a same-origin
`Referer` fallback), rotates the Shared Auth refresh token, delegates a fresh
short-lived token for `aud=zed-pkg`, `azp=zpkg-web`, `scope=zpkg:account`, and
forwards JSON to the canonical `/api/v1/account/*` route.

The registry Postgres identity used by this web process remains read-only. All
writes still pass through the API's canonical SeaORM write context and database
policy triggers, including the 10-day/50-download private-to-public invariant.

The API never redirects a browser. It requires an active delegated credential
with the exact product contract:

```text
aud = zed-pkg
azp = zpkg-web
scope contains zpkg:account
sid is present
parent_jti is present
```

Shared Auth owns authentication, assurance, base-session rotation, and
revocation. Zpkg owns organizations, projects, memberships, invitations,
package visibility, package roles, publication rights, and all registry data
policy.

## Failure semantics

A missing or invalid product cookie redirects a browser action back through the
exact-client login flow. A state, callback, client, origin, principal, audience,
authorized-party, or scope mismatch fails closed. Shared Auth and registry API
transport failures are reported as upstream failures rather than being treated
as anonymous or successful mutations.

Refresh rotation is persisted back into the signed product cookie on successful
API writes and API-level error responses. A refreshed principal different from
the cookie principal is rejected.

## Required configuration

| Variable | Purpose |
| --- | --- |
| `PUBLIC_BASE_URL` | exact app origin, e.g. `https://app.zpkg.net` |
| `SHARED_AUTH_URL` | cluster-internal Shared Auth origin |
| `SHARED_AUTH_PUBLIC_URL` | browser-visible Shared Auth origin, which may be on a different domain |
| `SHARED_AUTH_HANDOFF_CLIENT_ID` | exact handoff client, default `zpkg` |
| `SHARED_AUTH_HANDOFF_CLIENT_SECRET` | backend-only handoff redemption secret |
| `SHARED_AUTH_DELEGATE_CLIENT_ID` | delegated-token authorized party, default `zpkg-web` |
| `SHARED_AUTH_AUDIENCE` | delegated audience, default `zed-pkg` |
| `SHARED_AUTH_SCOPES` | comma/space-delimited scopes; must include `zpkg:account` |
| `ZED_API_URL` | cluster-internal API origin |
| `ZED_SESSION_SIGNING_SECRET` | dedicated cookie HMAC key, at least 32 bytes |

No Supabase service-role key, Shared Auth client secret, delegated registry API
bearer token, database writer credential, or R2 secret belongs in browser
JavaScript or repository plaintext.

## Follow-up hardening

The current handoff returns a verified provider access token to this backend,
which then calls Shared Auth exchange and delegation endpoints. The preferred
hardening target is one atomic redeem-to-Shared-Auth-session operation that
consumes the code, verifies/exchanges the provider token, creates the base
session, and optionally returns the first delegated Zpkg token. That would keep
Supabase refresh material entirely inside Shared Auth, even on the internal
network.

This hardening target must not be represented as deployed until its code,
schema, refresh/reuse behavior, and exact-head integration tests are green.
