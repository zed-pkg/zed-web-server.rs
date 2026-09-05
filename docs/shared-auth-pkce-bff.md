# Shared Auth PKCE browser boundary

`app.zpkg.net` is a backend-for-frontend for browser authentication and account mutations. The browser is redirected to the exact registered Shared Auth client, returns only an authorization code and state to `/auth/shared/callback`, and never receives the delegated Zed API token.

## Login transaction

1. `/auth/sign-in` creates a high-entropy state, PKCE verifier, S256 challenge, and sanitized same-origin return path.
2. The state/verifier/return path are sealed into a short-lived, signed, origin-scoped login cookie.
3. Shared Auth authenticates the Supabase identity and redirects to the exact callback with a one-time code.
4. The web backend verifies state and redeems the code with the verifier and client secret over the private Shared Auth URL.
5. It requests a narrow `zpkg:account` delegated token for the API audience and establishes the API-owned account projection. Only the rotating opaque refresh handle, canonical principal, and issuance/verification times enter the signed application session cookie; the delegated bearer stays server-side. It deletes the login cookie and redirects only to the sanitized local path.

Cookies use `__Host-` names and `Secure` in HTTPS environments, `HttpOnly`, `SameSite=Lax`, path `/`, bounded lifetime, and signed authenticated contents. Logout and every mutation require a same-origin `Origin` or `Referer` before the session is delegated or forwarded.

## Proxy security headers

Security headers emitted by Shared Auth are authoritative for proxied pages. The site middleware inserts CSP, HSTS, frame, content-type, and referrer defaults only when the upstream omitted that header. This prevents a permissive site default from replacing a stricter authentication-page policy.

## Data and privilege boundary

The web server imports only the default read-only `zed-orm-core` surface and runs under a SELECT-only database principal. All writes are same-origin BFF calls to the API's canonical `/api/v1/account` routes. A release candidate must prove login, callback replay rejection, bad-state rejection, open-redirect rejection, cross-origin mutation rejection, session tamper rejection, logout, private-resource isolation, and preservation of stricter upstream security headers.

## Individual and organization entry

`/onboarding` lets a visitor choose `/onboarding/individual` or
`/onboarding/organization`. Both use the existing customer Shared Auth realm;
the selected path survives the sign-in ceremony. An organization is a product
workspace, not a second identity provider or an administrative realm. Employee
access requires an explicit membership or invitation; email-domain equality
does not grant authority.

Authenticated users explicitly choose a workspace. Personal users can browse
without creating an organization; publishing a namespace is opt-in. Namespace
and organization creation both call the canonical API operation that creates
the organization and initial owner membership transactionally. The browser
cannot submit its own principal or owner role. A missing or failed account
projection is an unavailable state, not proof that the user has no memberships.

Account pages, form errors, and redirects are `private, no-store`. Forms have
a 4 KiB extraction limit and retain escaped values on safe error pages. Failed
upstream bodies are not rendered. Customer organization administration does
not confer access to the separately isolated platform-admin application.

## Typed mutation and continuity outcomes

`BrowserMutation::{Applied, SignIn, Failed}` keeps API-confirmed success
separate from authentication redirects even when both use HTTP 303. Every
browser authority path uses the same checked session-age calculation, rejecting
expired, future-dated, malformed, and overflowing issuance times before refresh.
Principal changes during refresh prevent delegation and API calls. Once a
same-principal refresh has succeeded, a downstream failure preserves its new
opaque refresh handle. Session-status delegation outages return unavailable
with the rotated cookie; revocation instead clears the session.

Rotation time and verification time are separate. Only successful delegation
sets `verified_at`, which permits the coarse session-status fast path. A
rotated-but-unverified cookie, a legacy cookie without that field, or an invalid
verification timestamp must recheck the authority. An outage cannot manufacture
recent authentication evidence for the next probe.

The executable scope, assumptions, trace replay, and negative controls are
documented in [formal/README.md](../formal/README.md).
