//! zed-pkg registry web UI — app.zpkg.net.
//!
//! The library owns the MASH application and its process runtime. The binary
//! entrypoint remains a minimal Tokio adapter so startup policy can be tested
//! without coupling it to executable glue.
//!
//! This tier is **read-only by construction** at the database boundary. Browser
//! mutations terminate at a same-origin BFF route, which refreshes/delegates a
//! Shared Auth session and forwards the operation to the write-enabled API.
//!
//! ## Web/API data boundary
//!
//! Choose a route per operation; a query being read-only does not by itself make
//! it safe, cheap, authorized, or consistent.
//!
//! 1. **Direct database read — narrow SSR optimization.** Use only for a bounded
//!    list/detail projection when removing the API hop has a measured benefit.
//!    The web process must receive a distinct `__web_ro` principal with
//!    database-enforced `SELECT` allowlists, read-only transactions, timeouts,
//!    tenant/actor context, result limits, and negative write/isolation tests.
//!    Expose named `zed-lib-core` reads rather than a raw SeaORM connection.
//!    Never use this route for product-domain writes or across an
//!    untrusted/remote network boundary.
//! 2. **Stateless HTTP to the API cluster — default.** Use the generated client
//!    for every product mutation and for authorization-sensitive, composite,
//!    rapidly evolving, or consistency-sensitive reads. Call the load-balanced
//!    service endpoint with deadlines and trace/actor context. HTTP connection
//!    pooling may reuse TCP/QUIC underneath, but no application session belongs
//!    to a particular socket or replica. Retry only idempotent operations or
//!    mutations protected by an idempotency key.
//! 3. **Application-stateful TCP to the API cluster — streaming exception.** A
//!    framed TCP session is appropriate only for measured high-rate streaming
//!    or backpressure/resume requirements that HTTP streaming or WebSockets do
//!    not satisfy. Authenticate the connection and each logical stream, bound
//!    buffers and heartbeats, support versioned framing and resume tokens, and
//!    reconnect through a TCP-aware load balancer so deploys and failed replicas
//!    do not strand state. Do not invent a custom TCP protocol for ordinary
//!    form, CRUD, registry, or SSR traffic.
//! 4. **NATS/message queue — asynchronous workflow.** Publish a typed, versioned
//!    command/event for durable jobs, fan-out, or work whose result is not needed
//!    to render the current response. Include actor/tenant context, correlation
//!    and idempotency keys, expiry, retry/dead-letter policy, and an audit trail.
//!    Persist the result and notify the browser by polling, SSE, or WebSocket.
//!    Broker request/reply is not the default synchronous RPC path.
//!
//! Location guide: browser/edge traffic uses HTTPS; an ordinary in-cluster web
//! handler uses stateless HTTP; a same-trust-zone SSR hot path may earn a
//! constrained direct read; a genuinely sessionful high-volume stream may earn
//! TCP; and background processing or integration events use NATS. After a
//! mutation, render from the API response, an API primary read, or an explicit
//! consistency token rather than assuming a read replica is current.
//!
//! `zed-api-server` owns product-domain mutations, transaction invariants,
//! idempotency, auditing, and event publication. This web server may write only
//! isolated web-owned state such as encrypted sessions, PKCE/CSRF state, and a
//! bounded render cache. `zed-lib-core` owns desired SQL, persistence schema/JSON,
//! reviewed migration inputs, generated SeaORM adapters, and named operations;
//! `zed-interfaces` owns public wire contracts. Production DDL runs only from a
//! serialized one-shot `__migrator` job. Web/API replicas never auto-migrate at
//! startup, and code-first models in a server crate never become a second schema
//! authority.

pub mod four_transports;
pub mod web_api_plane;
mod browser_auth;
mod proxy;
mod routes;
pub mod server;
mod session;
mod state;
mod views;
