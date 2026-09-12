//! Executable configuration admission for the web server.
//!
//! `.cli-flags.toml` is the sole public argv/default/type authority. The
//! bundled flags2env parser audits that contract before any tracing, network,
//! database, auth, or listener effect. Secret-bearing values are sampled once
//! from the process environment and never become argv flags.

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use flags2env::{BundledFlags2Env, StructuredParse};
use serde::Deserialize;

const CONTRACT_PATH: &str = ".cli-flags.toml";

const PUBLIC_ENV_KEYS: &[&str] = &[
    "CLI_HELP_REQUESTED",
    "CLI_VERSION_REQUESTED",
    "BIND_ADDR",
    "RUST_LOG",
    "DB_MAX_CONNECTIONS",
    "DB_STATEMENT_TIMEOUT_MS",
    "DB_CONNECT_MAX_WAIT_SECS",
    "PUBLIC_BASE_URL",
    "SHARED_AUTH_URL",
    "SHARED_AUTH_PUBLIC_URL",
    "ZED_API_URL",
    "PUBLIC_REGISTRY_URL",
    "SHARED_AUTH_HANDOFF_CLIENT_ID",
    "SHARED_AUTH_DELEGATE_CLIENT_ID",
    "SHARED_AUTH_AUDIENCE",
    "SHARED_AUTH_SCOPES",
];

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct PublicRuntimeConfig {
    #[serde(rename = "CLI_HELP_REQUESTED", default)]
    pub(crate) help_requested: bool,
    #[serde(rename = "CLI_VERSION_REQUESTED", default)]
    pub(crate) version_requested: bool,
    #[serde(rename = "BIND_ADDR")]
    pub(crate) bind_addr: String,
    #[serde(rename = "RUST_LOG")]
    pub(crate) rust_log: String,
    #[serde(rename = "DB_MAX_CONNECTIONS")]
    pub(crate) db_max_connections: u32,
    #[serde(rename = "DB_STATEMENT_TIMEOUT_MS")]
    pub(crate) db_statement_timeout_ms: u32,
    #[serde(rename = "DB_CONNECT_MAX_WAIT_SECS")]
    pub(crate) db_connect_max_wait_secs: u64,
    #[serde(rename = "PUBLIC_BASE_URL")]
    pub(crate) public_base_url: String,
    #[serde(rename = "SHARED_AUTH_URL", default)]
    pub(crate) shared_auth_url: Option<String>,
    #[serde(rename = "SHARED_AUTH_PUBLIC_URL", default)]
    pub(crate) shared_auth_public_url: Option<String>,
    #[serde(rename = "ZED_API_URL")]
    pub(crate) zed_api_url: String,
    #[serde(rename = "SHARED_AUTH_HANDOFF_CLIENT_ID")]
    pub(crate) handoff_client_id: String,
    #[serde(rename = "SHARED_AUTH_DELEGATE_CLIENT_ID")]
    pub(crate) delegate_client_id: String,
    #[serde(rename = "SHARED_AUTH_AUDIENCE")]
    pub(crate) audience: String,
    #[serde(rename = "SHARED_AUTH_SCOPES")]
    pub(crate) scopes: String,
}

/// Immutable startup snapshot. This type intentionally does not implement
/// `Debug`: three fields may contain credential material.
pub(crate) struct RuntimeConfig {
    pub(crate) public: PublicRuntimeConfig,
    pub(crate) database_url: Option<String>,
    pub(crate) handoff_client_secret: Option<String>,
    pub(crate) session_signing_secret: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartupIntent {
    Serve,
    Help,
    Version,
}

impl RuntimeConfig {
    pub(crate) fn startup_intent(&self) -> Result<StartupIntent> {
        startup_intent(
            self.public.help_requested,
            self.public.version_requested,
        )
    }
}

pub(crate) fn resolve_process() -> Result<RuntimeConfig> {
    let argv = std::env::args().collect::<Vec<_>>();
    let process_env = std::env::vars().collect::<HashMap<_, _>>();
    resolve_with_contract(&argv, &process_env, CONTRACT_PATH)
}

fn resolve_with_contract(
    argv: &[String],
    process_env: &HashMap<String, String>,
    contract_path: &str,
) -> Result<RuntimeConfig> {
    let parser = BundledFlags2Env::new();
    parser
        .audit_config(Some(contract_path))
        .map_err(|_| anyhow!("flags2env startup contract audit failed"))?;
    let parsed = parser
        .parse_structured(argv, Some(contract_path))
        .map_err(|_| anyhow!("flags2env could not resolve startup arguments"))?;

    if !parsed.errors.is_empty()
        || !parsed.unknown_options.is_empty()
        || !parsed.extras.is_empty()
        || !parsed.command.is_empty()
        || !parsed.subcommands.is_empty()
    {
        bail!("startup arguments were rejected by the flags2env contract");
    }

    let values = merge_public_sources(parsed, process_env);
    let public = parser
        .coerce::<PublicRuntimeConfig, _>(&values, Some(contract_path))
        .map_err(|_| anyhow!("typed public startup configuration is invalid"))?;

    Ok(RuntimeConfig {
        public,
        database_url: process_env.get("DATABASE_URL").cloned(),
        handoff_client_secret: process_env
            .get("SHARED_AUTH_HANDOFF_CLIENT_SECRET")
            .cloned(),
        session_signing_secret: process_env.get("ZED_SESSION_SIGNING_SECRET").cloned(),
    })
}

fn merge_public_sources(
    parsed: StructuredParse,
    process_env: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut values = parsed.flags;
    values.extend(parsed.dotenv);
    for key in PUBLIC_ENV_KEYS {
        if let Some(value) = process_env.get(*key) {
            values.insert((*key).to_owned(), value.clone());
        }
    }
    values.extend(parsed.dotenv_overrides);
    values.extend(parsed.provided_flags);
    values
}

fn startup_intent(help: bool, version: bool) -> Result<StartupIntent> {
    match (help, version) {
        (false, false) => Ok(StartupIntent::Serve),
        (true, false) => Ok(StartupIntent::Help),
        (false, true) => Ok(StartupIntent::Version),
        (true, true) => bail!("--help and --version are mutually exclusive"),
    }
}

pub(crate) fn help_text() -> &'static str {
    "zed-web-server — read-only zed-pkg registry web UI\n\n\
Public options are admitted by .cli-flags.toml. Common options:\n\
  --bind-addr=<addr>\n\
  --rust-log=<filter>\n\
  --db-max-connections=<n>\n\
  --db-statement-timeout-ms=<ms>\n\
  --db-connect-max-wait-secs=<seconds>\n\
  --public-base-url=<origin>\n\
  --shared-auth-url=<url>\n\
  --zed-api-url=<url>\n\
  --help\n\
  --version\n\n\
Credential-bearing inputs remain environment-only.\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(items: &[(&str, &str)]) -> HashMap<String, String> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn source_merge_preserves_contract_precedence() {
        let parsed = StructuredParse {
            flags: map(&[("BIND_ADDR", "default"), ("RUST_LOG", "info")]),
            dotenv: map(&[("BIND_ADDR", "dotenv")]),
            dotenv_overrides: map(&[("RUST_LOG", "dotenv-override")]),
            provided_flags: map(&[("BIND_ADDR", "argv")]),
            ..StructuredParse::default()
        };
        let process = map(&[("BIND_ADDR", "process"), ("RUST_LOG", "debug")]);
        let merged = merge_public_sources(parsed, &process);
        assert_eq!(merged.get("BIND_ADDR").map(String::as_str), Some("argv"));
        assert_eq!(
            merged.get("RUST_LOG").map(String::as_str),
            Some("dotenv-override")
        );
    }

    #[test]
    fn secret_process_values_are_not_merged_into_public_coercion_input() {
        let parsed = StructuredParse {
            flags: map(&[("BIND_ADDR", "127.0.0.1:8081")]),
            ..StructuredParse::default()
        };
        let process = map(&[
            ("DATABASE_URL", "synthetic-secret-db"),
            ("SHARED_AUTH_HANDOFF_CLIENT_SECRET", "synthetic-secret-auth"),
            ("ZED_SESSION_SIGNING_SECRET", "synthetic-secret-session"),
        ]);
        let merged = merge_public_sources(parsed, &process);
        assert!(!merged.contains_key("DATABASE_URL"));
        assert!(!merged.contains_key("SHARED_AUTH_HANDOFF_CLIENT_SECRET"));
        assert!(!merged.contains_key("ZED_SESSION_SIGNING_SECRET"));
    }

    #[test]
    fn startup_control_flags_are_mutually_exclusive() {
        assert_eq!(startup_intent(false, false).unwrap(), StartupIntent::Serve);
        assert_eq!(startup_intent(true, false).unwrap(), StartupIntent::Help);
        assert_eq!(startup_intent(false, true).unwrap(), StartupIntent::Version);
        assert!(startup_intent(true, true).is_err());
    }

    #[test]
    fn contract_keeps_dual_no_dotenv_isolation_and_secret_ignores() {
        let contract = include_str!("../.cli-flags.toml");
        assert!(contract.lines().any(|line| line.trim() == "dotenv = false"));
        assert!(contract.lines().any(|line| line.trim() == "files = []"));
        for key in [
            "DATABASE_URL",
            "SHARED_AUTH_HANDOFF_CLIENT_SECRET",
            "SHARED_AUTH_SERVICE_CREDENTIAL",
            "ZED_SESSION_SIGNING_SECRET",
        ] {
            assert!(contract.contains(&format!("\"{key}\"")));
        }
    }
}
