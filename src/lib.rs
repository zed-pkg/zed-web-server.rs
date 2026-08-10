//! zed-pkg registry web UI — app.zpkg.net.
//!
//! The library owns the MASH application and its process runtime. The binary
//! entrypoint remains a minimal Tokio adapter so startup policy can be tested
//! without coupling it to executable glue.
//!
//! This tier is **read-only by construction** at the database boundary. Browser
//! mutations terminate at a same-origin BFF route, which refreshes/delegates a
//! Shared Auth session and forwards the operation to the write-enabled API.

mod browser_auth;
mod marketing_session;
mod proxy;
mod routes;
pub mod server;
mod session;
mod state;
mod views;