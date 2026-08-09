//! zed-pkg registry web UI — app.zpkg.net.
//!
//! The library owns the MASH application and its process runtime. The binary
//! entrypoint remains a minimal Tokio adapter so startup policy can be tested
//! without coupling it to executable glue.
//!
//! This tier is **read-only by construction**: its database identity holds only
//! SELECT, and it depends on `zed-orm-core` with default features, so the
//! compiler refuses to give it a write context at all. Every mutation the UI
//! offers is a form that posts to the API server.

mod proxy;
mod routes;
pub mod server;
mod session;
mod state;
mod views;
