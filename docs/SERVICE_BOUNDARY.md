# Web/API service boundary

The registry web experience renders HTML and browser flows; `zed-api-server.rs`
owns versioned registry JSON, publication authorization, package mutation, and
the role-aware data plane. The web UI imports `zed-interfaces` instead of
recreating registry models.

| Connection | Allowed use | Boundary |
| --- | --- | --- |
| Direct database read | public, cache-safe package/catalog projection only | no publish, token, organization, or private package state |
| Stateless HTTP/JSON | default for search, account, publish, and admin intents | API validates identity and authorizes every action again |
| Stateful TCP | progress/subscription updates that already have HTTP authorization | no publish request/response or durable cursor authority |
| NATS/MQ | immutable post-commit package indexing, audit, and notification effects | never package acceptance, version allocation, or authorization |

Use an outbox-backed event only after the registry transaction commits. A web
request receives the API's committed result; it never waits on a message broker
to decide whether an immutable package version exists.
