# Shared Auth PKCE browser boundary

`app.zpkg.net` is a backend-for-frontend for browser authentication and account mutations. The browser is redirected to the exact registered Shared Auth client, returns only an authorization code and state to `/auth/shared/callback`, and never receives the delegated Zed API token.

## Login transaction

1. `/auth/sign-in` creates a high-entropy state, PKCE verifier, S256 challenge, and sanitized same-origin return path.
2. The state/verifier/return path are sealed into a short-lived, signed, origin-scoped login cookie.
3. Shared Auth authenticates the Supabase identity and redirects to the exact callback with a one-time code.
4. The web backend verifies state and redeems the code with the verifier and client secret over the private Shared Auth URL.
5. It requests a narrow `zpkg:account` delegated token for the API audience, seals that token and subject into the application session cookie, deletes the login cookie, and redirects only to the sanitized local path.

Cookies use `__Host-` names and `Secure` in HTTPS environments, `HttpOnly`, `SameSite=Lax`, path `/`, bounded lifetime, and signed authenticated contents. Logout and every mutation require a same-origin `Origin` or `Referer` before the session is delegated or forwarded.

## Proxy security headers

Security headers emitted by Shared Auth are authoritative for proxied pages. The site middleware inserts CSP, HSTS, frame, content-type, and referrer defaults only when the upstream omitted that header. This prevents a permissive site default from replacing a stricter authentication-page policy.

## Data and privilege boundary

The web server imports only the default read-only `zed-orm-core` surface and runs under a SELECT-only database principal. All writes are same-origin BFF calls to the API's canonical `/api/v1/account` routes. A release candidate must prove login, callback replay rejection, bad-state rejection, open-redirect rejection, cross-origin mutation rejection, session tamper rejection, logout, private-resource isolation, and preservation of stricter upstream security headers.
