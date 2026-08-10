# Marketing session contract

The static `zpkg.net` site never reads a JWT, Shared Auth access token, delegated
registry bearer, or opaque refresh handle. It uses credentialed same-site
requests to the Rust BFF at `app.zpkg.net`:

- `GET /auth/session/status` returns only `{ authenticated, refreshAfterSeconds }`.
- `POST /auth/session/refresh` rotates the opaque Shared Auth refresh handle
  inside the signed, HttpOnly, host-only product cookie.
- Credentialed CORS is emitted only for the exact `https://zpkg.net` origin;
  wildcard origins are forbidden.
- Responses are `no-store` and include no principal, tenant, or account data.
- Web clients check on initial load, focus, visibility, and online recovery,
  with a 50-minute foreground timer and best-effort Periodic Background Sync.
- Mobile clients use the same 50-minute target only when the OS grants
  background execution and always refresh on foreground/resume.

The status endpoint is deliberately not a token introspection API. A failed
network request leaves the marketing header in its neutral `Account` state, and
an invalid signed cookie is cleared.

Production activation remains gated on exact-head Shared Auth PKCE/handoff
certification and client/secret provisioning. No static-site deploy should
receive confidential client or Supabase service credentials.
