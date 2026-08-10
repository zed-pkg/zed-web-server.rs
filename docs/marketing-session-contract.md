# Marketing session status and refresh

`zpkg.net` is static. Authentication terminates at the Rust BFF on
`app.zpkg.net`, which uses the exact-client Shared Auth PKCE flow and keeps only
an opaque refresh handle plus canonical principal ID in a signed, host-only,
`Secure; HttpOnly; SameSite=Lax` cookie.

The marketing site calls two exact-origin endpoints with `credentials: include`:

- `GET /auth/session/status` returns only
  `{ "authenticated": boolean, "refreshAfterSeconds": 3000 }`.
- `POST /auth/session/refresh` rotates the Shared Auth session on the server,
  replaces the signed cookie, and returns the same token-blind shape.

Neither response includes a JWT, opaque refresh handle, principal, email,
tenant, or account metadata. Responses are `no-store`; credentialed CORS is
emitted only for `https://zpkg.net`. The rest of the BFF's mutations retain
their app-origin-only CSRF checks.

The browser performs an initial status read, refreshes authenticated sessions at
the 50-minute mark, and recovers on focus, visibility, and online events.
Periodic Background Sync is best effort only. Mobile clients should use the same
50-minute target when the operating system grants background execution, and
must always refresh on foreground/resume.
