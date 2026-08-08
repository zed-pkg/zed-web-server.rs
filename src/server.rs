//! Process composition for the registry web server.
//!
//! The API server remains the schema owner. This read-only web process may
//! start without Postgres and retry for a bounded interval before entering its
//! existing offline mode.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sea_orm::{DatabaseConnection, SqlxPostgresConnector};
use tracing_subscriber::EnvFilter;

use crate::state::WebState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DatabaseStartupPolicy {
    max_connections: u32,
    statement_timeout_ms: u32,
    max_wait: Duration,
}

impl DatabaseStartupPolicy {
    fn from_values(
        max_connections: Option<&str>,
        statement_timeout_ms: Option<&str>,
        max_wait_secs: Option<&str>,
    ) -> Self {
        Self {
            max_connections: parse_u32_or(max_connections, 10),
            statement_timeout_ms: parse_u32_or(statement_timeout_ms, 8_000),
            max_wait: Duration::from_secs(parse_u64_or(max_wait_secs, 30)),
        }
    }

    fn from_env() -> Self {
        let max_connections = std::env::var("DB_MAX_CONNECTIONS").ok();
        let statement_timeout_ms = std::env::var("DB_STATEMENT_TIMEOUT_MS").ok();
        let max_wait_secs = std::env::var("DB_CONNECT_MAX_WAIT_SECS").ok();
        Self::from_values(
            max_connections.as_deref(),
            statement_timeout_ms.as_deref(),
            max_wait_secs.as_deref(),
        )
    }
}

fn parse_u32_or(value: Option<&str>, default: u32) -> u32 {
    value
        .and_then(|candidate| candidate.parse::<u32>().ok())
        .unwrap_or(default)
}

fn parse_u64_or(value: Option<&str>, default: u64) -> u64 {
    value
        .and_then(|candidate| candidate.parse::<u64>().ok())
        .unwrap_or(default)
}

/// `SHARED_AUTH_URL` enables the /shared-auth gateway path. Trailing slashes
/// are trimmed so joining with the stripped request path cannot double a `/`;
/// unset or empty leaves the gateway disabled (those routes answer 503).
fn shared_auth_url(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim_end_matches('/');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Open and verify one Postgres pool under the reviewed startup policy.
async fn try_connect(url: &str, policy: DatabaseStartupPolicy) -> Result<DatabaseConnection> {
    let connect = url
        .parse::<PgConnectOptions>()?
        .options([("statement_timeout", policy.statement_timeout_ms.to_string())]);
    let pool = PgPoolOptions::new()
        .max_connections(policy.max_connections)
        .acquire_timeout(Duration::from_secs(8))
        .connect_with(connect)
        .await?;
    Ok(SqlxPostgresConnector::from_sqlx_postgres_pool(pool))
}

/// Retry the initial Postgres connection until the bounded startup deadline.
///
/// A read-only web pod normally starts alongside its database. If the deadline
/// expires, the existing fail-open behavior is preserved and the UI serves in
/// registry-offline mode.
async fn connect_with_retry(
    url: &str,
    policy: DatabaseStartupPolicy,
) -> Option<DatabaseConnection> {
    let started = Instant::now();
    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        match try_connect(url, policy).await {
            Ok(database) => {
                if attempt > 1 {
                    tracing::info!(attempt, "connected to Postgres after retry");
                }
                return Some(database);
            }
            Err(error) if started.elapsed() >= policy.max_wait => {
                tracing::warn!(
                    %error,
                    attempts = attempt,
                    elapsed_s = started.elapsed().as_secs(),
                    "Postgres unreachable within DB_CONNECT_MAX_WAIT_SECS; serving in offline mode"
                );
                return None;
            }
            Err(error) => {
                tracing::warn!(%error, attempt, "Postgres not ready yet; retrying in 2s");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

/// Run the read-only MASH registry UI.
///
/// # Errors
///
/// Returns an error when the listener cannot bind or the HTTP server exits
/// unexpectedly. Database unavailability retains the product's reviewed
/// offline-mode behavior and is not a process-startup error.
pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let policy = DatabaseStartupPolicy::from_env();
    let database = match std::env::var("DATABASE_URL") {
        Ok(url) => connect_with_retry(&url, policy).await,
        Err(_) => {
            tracing::warn!("DATABASE_URL not set; serving in offline mode");
            None
        }
    };

    let state = Arc::new(WebState {
        db: database,
        registry_url: std::env::var("PUBLIC_REGISTRY_URL")
            .unwrap_or_else(|_| zed_interfaces::registry::DEFAULT_REGISTRY_URL.to_string()),
        shared_auth_url: shared_auth_url(std::env::var("SHARED_AUTH_URL").ok().as_deref()),
        http: crate::proxy::client(),
    });
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8081".to_string());

    let app = crate::routes::router(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("zed-web-server listening on {bind_addr}");
    // ConnectInfo feeds the gateway's X-Forwarded-For append.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_policy_defaults_preserve_the_existing_runtime_contract() {
        assert_eq!(
            DatabaseStartupPolicy::from_values(None, None, None),
            DatabaseStartupPolicy {
                max_connections: 10,
                statement_timeout_ms: 8_000,
                max_wait: Duration::from_secs(30),
            }
        );
    }

    #[test]
    fn database_policy_accepts_explicit_values_independently() {
        assert_eq!(
            DatabaseStartupPolicy::from_values(Some("23"), Some("7123"), Some("41")),
            DatabaseStartupPolicy {
                max_connections: 23,
                statement_timeout_ms: 7_123,
                max_wait: Duration::from_secs(41),
            }
        );
    }

    #[test]
    fn malformed_database_policy_values_fall_back_independently() {
        assert_eq!(
            DatabaseStartupPolicy::from_values(Some("many"), Some("-1"), Some("later")),
            DatabaseStartupPolicy {
                max_connections: 10,
                statement_timeout_ms: 8_000,
                max_wait: Duration::from_secs(30),
            }
        );
    }

    #[test]
    fn shared_auth_url_trims_trailing_slashes_and_treats_empty_as_unset() {
        assert_eq!(shared_auth_url(None), None);
        assert_eq!(shared_auth_url(Some("")), None);
        assert_eq!(shared_auth_url(Some("///")), None);
        assert_eq!(
            shared_auth_url(Some("http://127.0.0.1:8120")),
            Some("http://127.0.0.1:8120".to_string())
        );
        assert_eq!(
            shared_auth_url(Some("http://127.0.0.1:8120//")),
            Some("http://127.0.0.1:8120".to_string())
        );
    }

    #[test]
    fn executable_remains_a_thin_tokio_adapter() {
        let main = include_str!("main.rs");
        assert!(main.lines().count() <= 6);
        for lifecycle_symbol in [
            "DatabaseConnection",
            "PgPoolOptions",
            "routes::router",
            "TcpListener::bind",
        ] {
            assert!(!main.contains(lifecycle_symbol));
        }
    }
}
