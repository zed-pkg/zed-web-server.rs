//! zed-pkg registry web UI.
//!
//! The library owns the MASH application and its process runtime. The binary
//! entrypoint remains a minimal Tokio adapter so startup policy can be tested
//! without coupling it to executable glue.

mod account;
mod entities;
mod proxy;
mod routes;
pub mod server;
mod state;
mod views;
