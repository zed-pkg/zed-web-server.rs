# Shared Auth integration modes for `app.zpkg.net`

Zed supports **two intentionally different browser authentication ceremonies**.
They solve different deployment problems and must remain independently testable.
A route that belongs to one mode must never be treated as a compatibility name
for the other mode.

The common invariants are:

- Shared Auth owns upstream identity verification, provider/Supabase integration,
  canonical principals, factors, refresh sessions, revocation, and delegation.
- Zed owns organizations, projects, packages, memberships, and every product
  authorization decision in the Zed database.
- The browser never receives a Zed registry bearer token, a Shared Auth client
  secret, a Supabase service-role credential, or a cluster-internal URL.
- `https://app.zpkg.net/auth/shared/callback` is a **Zed product callback** for
  the PKCE handoff. It is not a Supabase callback.
- Direct HTTP calls to Shared Auth are back-channel calls made by a trusted Zed
  server. They do not replace the browser step needed for interactive login.

## Choosing a ceremony

| Requirement | Same-origin proxied ceremony | Shared Auth PKCE/BFF handoff |
|---|---|---|
| Public entry point | `/shared-auth-ui/auth/browser/sign-in` | `/auth/sign-in` |
| Product callback | Not required | `/auth/shared/callback` |
| Browser leaves `app.zpkg.net` | Usually no | Yes, for `/authorize`, then returns |
| Browser session owner | Shared Auth, with cookies issued through the Zed origin | Zed BFF, backed by a rotating opaque Shared Auth refresh handle |
| Confidential client secret | Not required for the browser ceremony | Required by the Zed backend for one-time-code redemption |
| Zed API token | Minted/delegated server-side when needed | Minted/delegated server-side when needed |
| Best fit | Simple first-party sign-in UI, origin-scoped Shared Auth session, low product-specific ceremony logic | Full account console, explicit product client boundary, same-origin mutations, narrow delegated API tokens |

Both modes may be enabled in one deployment. They use distinct route namespaces,
cookie names, transient state, and observability labels. A successful session in
one mode is not silently accepted as a session in the other mode.

## Mode A: same-origin proxied Shared Auth ceremony

The dedicated public prefix is `/shared-auth-ui`. The Zed router rewrites that
prefix to the existing internal `/shared-auth` proxy prefix, and the proxy strips
its prefix before forwarding to `shared-auth-server.rs`.

Configure Shared Auth with:

```text
AUTH_BROWSER_PUBLIC_PREFIX=/shared-auth-ui
```

The browser sequence is:

```text
Browser
  -> GET /shared-auth-ui/auth/browser/sign-in?return=/dashboard/acme
  -> zed-web-server.rs allow-listed proxy
  -> shared-auth-server.rs /auth/browser/sign-in
  -> magic-link, email-OTP, password, passkey, or other Shared Auth ceremony
  -> /shared-auth-ui/auth/browser/consume or /shared-auth-ui/auth/browser/otp
  -> Shared Auth sets host-only Secure HttpOnly cookies on app.zpkg.net
  -> local Zed return path
```

There is no `/auth/shared/callback` in this flow. Shared Auth owns the browser
ceremony and its provider-specific continuation. If a future upstream provider
requires a callback, that callback remains a Shared Auth route under the
`/shared-auth-ui` prefix; it does not become the Zed PKCE callback.

The dedicated proxy exposes only these service-local paths:

```text
/
/ui
/auth/browser/sign-in
/auth/browser/consume
/auth/browser/otp
```

It deliberately does **not** expose redemption, exchange, delegation, refresh,
introspection, metrics, or internal webhook endpoints. Those are back-channel
APIs and must use `SHARED_AUTH_URL` over the cluster network.

The existing broad `/shared-auth/*` gateway is retained only for compatibility
with already-reviewed pages and assets. New browser-ceremony integration must
use `/shared-auth-ui/*`; new server-to-server integration must use the internal
Shared Auth origin. Do not add new dependencies on the broad gateway.

## Mode B: Shared Auth PKCE handoff with a Zed BFF session

This is the account-console flow implemented by `src/browser_auth.rs`.

```text
Browser
  -> GET /auth/sign-in?return_to=/dashboard/acme
  -> zed-web-server.rs creates state + PKCE verifier
  -> 302 Shared Auth /authorize
  -> Shared Auth authenticates against the Supabase project registered to zpkg
  -> 302 /auth/shared/callback?code=...&state=...
  -> zed-web-server.rs validates state and the exact callback
  -> POST Shared Auth /auth/handoff/redeem       [back channel]
  -> POST Shared Auth /auth/exchange             [back channel]
  -> POST Shared Auth /auth/delegate             [back channel]
  -> GET Zed API /api/v1/account/me              [back channel]
  -> Zed sets its host-only Secure HttpOnly product cookie
  -> local Zed return path
```

Only a short-lived, single-use, PKCE-bound code and `state` cross the browser
front channel. Supabase access and refresh tokens remain inside Shared Auth and
the confidential redemption response. The Zed browser never sees the delegated
`aud=zed-pkg`, `azp=zpkg-web`, `scope=zpkg:account` token.

The Zed product cookie contains only the minimum BFF session material: the
rotating opaque Shared Auth refresh handle, the canonical Shared Auth principal
identifier, issue time, and an integrity signature. Every account mutation:

1. validates exact `Origin`, with a same-origin `Referer` fallback;
2. rotates the Shared Auth refresh handle;
3. delegates a short-lived Zed token;
4. forwards JSON to the canonical `/api/v1/account/*` route; and
5. persists the rotated handle even when the API returns an application error.

See [`../shared-auth-bff.md`](../shared-auth-bff.md) and
[`../shared-auth-pkce-bff.md`](../shared-auth-pkce-bff.md) for the lower-level
request and configuration contracts.

## Direct HTTP calls: web server versus API server

A direct HTTP call is correct after a trusted server has a credential or a
one-time code to validate. It is not an interactive-login substitute.

### `zed-web-server.rs`

The web BFF owns the PKCE state cookie, callback, handoff redemption, refresh
rotation, delegation, BFF logout, and same-origin form forwarding. It may call:

```text
POST /auth/handoff/redeem
POST /auth/exchange
POST /auth/refresh
POST /auth/delegate
POST /auth/logout
```

The same-origin proxied mode may also require the web server to validate the
Shared Auth session cookie before rendering protected product pages. That check
must be server-side and may use local JWKS verification or internal
introspection.

### `zed-api-server.rs`

The API server is a resource server, not the owner of the interactive browser
ceremony. It should:

- verify delegated JWT signatures and immutable claims locally against Shared
  Auth JWKS for normal requests; and
- use `/auth/introspect` over the cluster network when immediate session or
  revocation status is required.

Account routes require the exact delegated-token provenance:

```text
aud = zed-pkg
azp = zpkg-web
scope contains zpkg:account
sid is present
parent_jti is present
```

A deployment may choose to place the confidential BFF duties in an API process
instead of the web process, but exactly one component must own the state cookie,
callback redemption, product session, and refresh rotation. Do not split those
responsibilities across two processes without a transactional shared session
store.

## Route ownership

| Zed route | Owner | Meaning |
|---|---|---|
| `/auth/sign-in` | Zed BFF | Start PKCE handoff |
| `/auth/shared/callback` | Zed BFF | Validate state and redeem one-time code |
| `/auth/logout` | Zed BFF | Revoke and clear the BFF session |
| `/shared-auth-ui/*` | Shared Auth through a narrow Zed proxy | Same-origin Shared Auth-owned browser ceremony |
| `/shared-auth/auth/browser/sign-in` | Zed BFF compatibility alias | PKCE/BFF start; **not** the proxied ceremony |
| `/shared-auth/auth/logout` | Zed BFF compatibility alias | BFF logout; **not** a raw Shared Auth logout |
| `/shared-auth/*` | Legacy broad proxy | Compatibility only; do not use for new confidential API calls |
| `/v1/*` | Zed BFF | Same-origin mutation facade forwarding to `/api/v1/account/*` |

## Cookie separation

Production cookies use the `__Host-` prefix, `Path=/`, `Secure`, `HttpOnly`, and
an appropriate `SameSite` policy. The modes must use different names.

A representative namespace is:

```text
__Host-zpkg-shared-auth        # Shared Auth-owned same-origin access/session cookie
__Host-zpkg-shared-auth-refresh
__Host-zpkg-bff                # Zed BFF product session
__Host-zpkg-login              # short-lived PKCE state/verifier cookie
```

The configured names are authoritative; the examples above are not implicit
defaults. Never let the BFF parser accept the proxied-mode cookie or vice versa.

## Coexistence and logout semantics

- Both modes map to the same canonical Shared Auth principal and the same
  idempotent Zed account projection.
- Each mode may have a different Shared Auth session id. This is expected.
- Logout clears the active mode's product-origin cookies and revokes its refresh
  session. A separate explicit "log out everywhere" operation may revoke all
  sessions for the principal.
- A missing session for one mode does not fall back to the other mode. The route
  initiating a protected action selects the ceremony explicitly.
- Local return paths are allow-listed and relative. Absolute, protocol-relative,
  fragment-bearing, or header-injecting values fail closed.
- Trace and audit events should include `auth.ceremony=proxied` or
  `auth.ceremony=pkce_bff` plus the product client id, without token material.

## Required test matrix

### Same-origin proxy

- sign-in, magic-link consume, and OTP consume stay on `app.zpkg.net`;
- Shared Auth links and form actions use `/shared-auth-ui`;
- session cookies are host-only, Secure, HttpOnly, and use distinct names;
- `/shared-auth-ui/auth/handoff/redeem`, `/auth/exchange`, `/auth/delegate`,
  `/auth/introspect`, `/metrics`, and internal webhooks return 404;
- foreign `return` values and protocol-relative paths are rejected;
- product-page session validation fails closed when Shared Auth is unavailable.

### PKCE/BFF

- exact client id, callback URI, state, and S256 verifier are required;
- a handoff code is single-use, expires quickly, and cannot be redeemed by a
  different client or callback;
- provider tokens never appear in URLs, HTML, logs, or browser storage;
- delegated tokens contain the required audience, authorized party, scope,
  session, and parent-token lineage;
- cross-origin mutations and logout requests are rejected;
- refresh rotation is persisted on API success and API-level failure.

### Cross-mode

- cookies never collide or authenticate the other mode;
- both sessions project to the same canonical Zed user without duplicate rows;
- logging out of one mode has documented, tested behavior for the other mode;
- route aliases cannot accidentally switch ceremonies;
- browser E2E covers both modes against real Postgres and the exact Shared Auth
  and Zed server revisions intended for deployment.

## Implementation references

- [`../../src/routes/mod.rs`](../../src/routes/mod.rs): route ownership and the
  narrow `/shared-auth-ui` proxy surface.
- [`../../src/proxy.rs`](../../src/proxy.rs): HTTP forwarding and header/cookie
  preservation.
- [`../../src/browser_auth.rs`](../../src/browser_auth.rs): PKCE/BFF callback,
  refresh, delegation, and same-origin mutation facade.
- Shared Auth provider-side contract:
  `shared-auth/shared-auth-server.rs/docs/product-browser-auth/README.md`.
