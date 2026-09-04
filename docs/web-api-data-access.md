# Web/API data-access decision

Tracking: [DEN-3960](https://linear.app/denman/issue/DEN-3960/document-4-web-server-to-api-server-data-access-patterns-across-10)

Portfolio authority: [web-to-API data-access ADR](https://github.com/ORESoftware/k8s-cluster/blob/main/docs/architecture/web-api-data-access.md)

## Selected paths

Zed deliberately uses two synchronous paths, selected per operation:

- **P1, direct read-only database access**, is the default for server-rendered public and
  tenant-scoped registry views that fit the named `zed-orm-core` query surface. The web process gets
  only an opaque `ReadContext`; pool setup verifies `default_transaction_read_only=on`, caps pool
  and statement time, and fails to the explicit registry-offline UI rather than widening access.
- **P2, stateless HTTP to `zed-api-server`**, owns publish, account, administrative, and other
  mutations, plus reads whose authorization or consistency is not represented by a named P1 query.
  The BFF obtains an audience- and scope-bounded delegated credential, restricts the API origin and
  method, and keeps product credentials out of the browser.

The web server has no P3 connection to the Zed API. HTTP keep-alive remains P2. A future P3 protocol
is justified only for a sustained ordered stream and must define versioned frames, authorization at
handshake and expiry, operation IDs, bounded queues/connections, heartbeat, idle and maximum
lifetime, cursor resume, reconnect jitter, overload behavior, and deployment drain.

The web server has no NATS credential and does not publish P4 commands. Registry indexing, mirror
refresh, or other durable background work may use P4 only behind the API-owned boundary with a
versioned envelope, transactional outbox/inbox, stable operation ID, expiry, bounded delivery and
concurrency, dead-letter recovery, trace context, and durable result lookup.

## Operation map

| Operation | Path | Owner and contract |
| --- | --- | --- |
| Render approved registry/package/search views | P1 | Named `zed-orm-core` read with the verified SELECT-only role and bounded query policy |
| Serve while Postgres is unavailable or misconfigured | P1 failure mode | Offline UI; never substitute an API/migrator credential |
| Read account/private API resources through the BFF | P2 | Delegated credential, allowlisted API origin, GET/HEAD only for the read helper |
| Publish, account, settings, and administrative mutations | P2 | `zed-api-server` owns authorization, invariant, transaction, and idempotency result |
| Future sustained API subscription | P3 only after protocol review | Cursor-resumable, bounded, authenticated, drainable connection |
| Future registry indexing/mirror job | P4 only through API | Durable idempotent command and queryable result; no web-side queue credential |

## Security, retry, backpressure, and observability

- `DATABASE_URL` must resolve to the exact `zed_pkg__web_ro`-style role. It has schema `USAGE` and
  an explicit `SELECT` allowlist, with no write, DDL, ownership, role-switch, or `BYPASSRLS`
  capability. Tenant and authorization scope are mandatory inputs to named private queries.
- P1 connection retry is bounded by `DB_CONNECT_MAX_WAIT_SECS`; pool size, acquire time, statement
  time, rows, and response size remain bounded. It then enters offline mode.
- P2 calls have bounded HTTP client timeouts and must stay inside the ingress deadline. Safe reads
  may retry with jitter. A mutation retries only with a stable idempotency key whose principal,
  operation, and normalized payload match the stored result.
- A P2 failure never falls back to P1 unless that operation is already declared as an authorized
  named P1 read. Neither path falls back to a queue or stateful stream.
- W3C trace context and an opaque correlation ID cross P2 and are linked to P1 query spans. Metrics
  use fixed operation/path/outcome labels, never tokens, package contents, tenant/user values, or
  high-cardinality identifiers.
- Shutdown stops new HTTP admission and bounds in-flight completion. P1 pools and P2 clients close
  normally; enabling P3 or P4 requires their additional drain contracts first.

## Schema and migration authority

The `rust-orm` slice of `zed-pkg/zed-lib-core` is the canonical `zed-orm-core` source and consumes
the Zed declarative schema contract. `zed-api-server` is the sole request-serving writer and owns the
release sequence. Reviewed DDL is generated and applied by a discrete migration job using a
separate migrator credential. This web process enables neither `read-write` nor `migrate`, exposes
no raw connection, synthesizes no DDL, and never runs shared migrations at startup.

The exact dependency, grant, compatibility, and offline-mode evidence remains in
[`database-boundary.md`](database-boundary.md).
