//! Read-only SeaORM entities mirroring the registry schema. The source of
//! truth is zed-api-server's `migration/` crate; keep these in sync with it.

pub mod org;
pub mod package;
pub mod version;
