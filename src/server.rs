//! Process composition for the registry web server.
//!
//! The API server remains the schema owner. This read-only web process may
//! start without Postgres and retry for a bounded interval before entering its
//! existing offline mode.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tracing_subscriber::EnvFilter;
use zed_orm_core::{ConnectPolicy, ReadContext};

use crate::state::{BrowserAuthConfig, WebState};

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

fn trimmed_base_url(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim().trim_end_matches('/');
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn required_env(name: &str) -> Result<String> {
    let value = std::env::var(name).with_context(|| format!("{name} is required"))?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("{name} cannot be empty");
    }
    Ok(value)
}

fn normalize_origin(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value).context("PUBLIC_BASE_URL must be an absolute URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("PUBLIC_BASE_URL must use http or https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("PUBLIC_BASE_URL must not contain credentials");
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        bail!("PUBLIC_BASE_URL must be an origin without a path, query, or fragment");
    }

    let host = url
        .host_str()
        .context("PUBLIC_BASE_URL must include a host")?;
    let host_without_ipv6_brackets = host
        .strip_prefix('[')
        .and_then(|candidate| candidate.strip_suffix(']'))
        .unwrap_or(host);
    let is_loopback = host_without_ipv6_brackets.eq_ignore_ascii_case("localhost")
        || host_without_ipv6_brackets
            .parse::<std::net::IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false);
    if url.scheme() == "http" && !is_loopback {
        bail!("PUBLIC_BASE_URL must use https outside loopback development");
    }

    Ok(url.origin().ascii_serialization())
}

fn parse_scopes(value: &str) -> Result<Vec<String>> {
    let mut scopes = Vec::new();
    for candidate in value
        .split(|character: char| character == ',' || character.is_ascii_whitespace())
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
    {
        if !scopes.iter().any(|scope| scope == candidate) {
            scopes.push(candidate.to_owned());
        }
    }
    if !scopes.iter().any(|scope| scope == "zpkg:account") {
        bail!("SHARED_AUTH_SCOPES must include zpkg:account");
    }
    Ok(scopes)
}

fn browser_auth_config(
    shared_auth_url: Option<String>,
    public_origin: &str,
) -> Result<Option<BrowserAuthConfig>> {
    let Some(shared_auth_url) = shared_auth_url else {
        return Ok(None);
    };
    let shared_auth_public_url =
        trimmed_base_url(std::env::var("SHARED_AUTH_PUBLIC_URL").ok().as_deref())
            .unwrap_or_else(|| shared_auth_url.clone());
    let session_signing_secret = required_env("ZED_SESSION_SIGNING_SECRET")?;
    if session_signing_secret.len() < 32 {
        bail!("ZED_SESSION_SIGNING_SECRET must contain at least 32 bytes");
    }
    let secure_cookies = public_origin.starts_with("https://");
    Ok(Some(BrowserAuthConfig {
        shared_auth_url,
        shared_auth_public_url,
        public_origin: public_origin.to_owned(),
        api_url: trimmed_base_url(std::env::var("ZED_API_URL").ok().as_deref())
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_owned()),
        handoff_client_id: std::env::var("SHARED_AUTH_HANDOFF_CLIENT_ID")
            .unwrap_or_else(|_| "zpkg".to_owned()),
        handoff_client_secret: required_env("SHARED_AUTH_HANDOFF_CLIENT_SECRET")?,
        delegate_client_id: std::env::var("SHARED_AUTH_DELEGATE_CLIENT_ID")
            .unwrap_or_else(|_| "zpkg-web".to_owned()),
        audience: std::env::var("SHARED_AUTH_AUDIENCE").unwrap_or_else(|_| "zed-pkg".to_owned()),
        scopes: parse_scopes(
            &std::env::var("SHARED_AUTH_SCOPES").unwrap_or_else(|_| "zpkg:account".to_owned()),
        )?,
        session_signing_secret,
        session_cookie_name: if secure_cookies {
            "__Host-zpkg_session".to_owned()
        } else {
            "zpkg_session".to_owned()
        },
        login_cookie_name: if secure_cookies {
            "__Host-zpkg_login".to_owned()
        } else {
            "zpkg_login".to_owned()
        },
        secure_cookies,
    }))
}

/// Open and verify one Postgres pool through the canonical opaque read seam.
///
/// The `zed-orm-core` boundary applies `default_transaction_read_only=on` to
/// every connection and verifies that PostgreSQL accepted the setting before
/// returning a `ReadContext` to application state.
async fn try_connect(url: &str, policy: DatabaseStartupPolicy) -> Result<ReadContext> {
    let connect_policy = ConnectPolicy::default()
        .with_max_connections(policy.max_connections)
        .with_acquire_timeout(Duration::from_secs(8))
        .with_statement_timeout_ms(policy.statement_timeout_ms);
    Ok(zed_orm_core::connect_read_only_with_policy(url, connect_policy).await?)
}

/// Retry the initial read-only Postgres connection until the bounded deadline.
///
/// An unavailable database or a failed read-only verification both preserve
/// the existing registry-offline behavior: availability may degrade, but the
/// browser-facing process never widens itself into a writer.
async fn connect_with_retry(url: &str, policy: DatabaseStartupPolicy) -> Option<ReadContext> {
    let started = Instant::now();
    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        match try_connect(url, policy).await {
            Ok(database) => {
                if attempt > 1 {
                    tracing::info!(attempt, "connected to read-only Postgres after retry");
                }
                return Some(database);
            }
            Err(error) if started.elapsed() >= policy.max_wait => {
                tracing::warn!(
                    %error,
                    attempts = attempt,
                    elapsed_s = started.elapsed().as_secs(),
                    "read-only Postgres unavailable or misconfigured within DB_CONNECT_MAX_WAIT_SECS; serving in offline mode"
                );
                return None;
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    attempt,
                    "read-only Postgres not ready or not read-only; retrying in 2s"
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

/// Run the read-only MASH registry UI.
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

    let public_origin = normalize_origin(
        &std::env::var("PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost:8081".to_owned()),
    )?;
    let shared_auth_url = trimmed_base_url(std::env::var("SHARED_AUTH_URL").ok().as_deref());
    let browser_auth = browser_auth_config(shared_auth_url.clone(), &public_origin)?;

    let state = Arc::new(WebState {
        db: database,
        // Existing Maud forms now target the same origin; the BFF routes below
        // translate their stable legacy paths to the canonical API hierarchy.
        registry_url: String::new(),
        shared_auth_url,
        session_path: "/auth/browser/session".to_owned(),
        browser_auth,
        http: crate::proxy::client(),
    });
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8081".to_owned());

    let app = crate::routes::router(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("zed-web-server listening on {bind_addr}");
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
    fn origins_are_exact_pathless_and_secure_outside_loopback() {
        assert_eq!(
            normalize_origin("https://app.zpkg.net/").unwrap(),
            "https://app.zpkg.net"
        );
        assert_eq!(
            normalize_origin("http://localhost:8081/").unwrap(),
            "http://localhost:8081"
        );
        assert_eq!(
            normalize_origin("http://127.0.0.1:8081/").unwrap(),
            "http://127.0.0.1:8081"
        );
        assert_eq!(
            normalize_origin("http://[::1]:8081/").unwrap(),
            "http://[::1]:8081"
        );
        assert!(normalize_origin("http://app.zpkg.net").is_err());
        assert!(normalize_origin("http://10.0.0.5:8081").is_err());
        assert!(normalize_origin("https://app.zpkg.net/path").is_err());
        assert!(normalize_origin("javascript:alert(1)").is_err());
        assert!(normalize_origin("https://user@app.zpkg.net").is_err());
    }

    #[test]
    fn account_scope_is_mandatory_and_deduplicated() {
        assert!(parse_scopes("packages:read").is_err());
        assert_eq!(
            parse_scopes("zpkg:account, zpkg:account packages:write").unwrap(),
            vec!["zpkg:account", "packages:write"]
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
