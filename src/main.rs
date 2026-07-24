mod entities;
mod routes;
mod state;
mod views;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sea_orm::{ConnectOptions, Database};
use tracing_subscriber::EnvFilter;

use crate::state::WebState;

/// MASH: Maud templates, Axum routing, SeaORM reads (never bare SQLx),
/// HTMX for live search. The API server owns the schema; this binary is a
/// read-only view over the same Postgres and runs fine without it
/// ("registry offline" mode) so the UI can boot with zero infrastructure.
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let db = match std::env::var("DATABASE_URL") {
        Ok(url) => match Database::connect(&url).await {
            Ok(db) => Some(db),
            Err(error) => {
                tracing::warn!(%error, "DATABASE_URL unreachable; serving in offline mode");
                None
            }
        },
        Err(_) => {
            tracing::warn!("DATABASE_URL not set; serving in offline mode");
            None
        }
    };

    let state = Arc::new(WebState {
        db,
        registry_url: std::env::var("PUBLIC_REGISTRY_URL")
            .unwrap_or_else(|_| zed_interfaces::registry::DEFAULT_REGISTRY_URL.to_string()),
    });
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8081".to_string());

    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("zed-web-server listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
