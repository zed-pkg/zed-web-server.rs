# Token-blind marketing session contract

The static `https://zpkg.net` site never reads a JWT, Shared Auth access token,
delegated registry bearer, opaque refresh handle, principal, or account field.
It sends a credentialed same-site request to the product-owned Rust endpoint:

```text
GET https://app.zpkg.net/auth/session/status
```

The response contains only `authenticated`, the canonical `dashboard_url`, and
the coarse `check_after_seconds` cadence. Responses are `no-store`, credentialed
CORS is granted only to the exact marketing origin, and wrong or missing origins
fail before session resolution.

The signed, HttpOnly, host-only product cookie remains the only browser session
carrier. Before the 50-minute cadence is due, the server validates its signature
and age locally. Once due, the same GET refreshes the opaque handle through Shared
Auth and verifies delegation for the configured client, audience, and scopes.
Successful revalidation rotates only the product cookie. Revoked, expired,
future-dated, wrong-principal, wrong-client, and wrong-audience sessions fail
closed and clear the cookie. A Shared Auth outage returns a token-blind `503`
without destroying a potentially recoverable cookie.

There is intentionally no cross-origin refresh mutation. The static client must
start in a neutral account state, check on load and foreground restoration, keep
one request in flight, and use the coarse cadence with jitter while the page is
open. On timeout, `503`, or another network error it must show safe login/signup
navigation rather than retaining stale authenticated UI.
