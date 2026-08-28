//! `/console/storage` — the artifact storage console.
//!
//! The web server holds no storage credential and never talks to an object
//! store. It asks the registry API, with the viewer's delegated token, for the
//! API's own provider-agnostic report and renders it. That is the whole reason
//! this page is not an embedded Cloudflare dashboard: the description is
//! Zed's, so it survives the bucket moving to S3 or Google Cloud Storage
//! without a line of UI changing.
//!
//! The decoders below mirror the API's wire contract. Both repositories carry
//! the same `contracts/storage-status.v1.json` fixture and assert their own
//! decoder accepts it, which keeps them agreeing without a shared dependency
//! and a coordinated version bump.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::browser_auth::{self, DelegatedGetOutcome};
use crate::session::{self, Viewer};
use crate::state::WebState;
use crate::views::storage::{
    Health, ObjectResult, ObjectVerdict, ObjectView, StorageView, page, unavailable,
};
use crate::views::{PageContext, layout};

/// Wire shapes, decoded exactly as the API serializes them. Unknown fields are
/// ignored on purpose: the API may add reporting without breaking this page.
#[derive(Debug, Deserialize)]
struct StatusPayload {
    backend: BackendPayload,
    health: HealthPayload,
    usage: UsagePayload,
    limits: LimitsPayload,
    observed_at: String,
}

#[derive(Debug, Deserialize)]
struct BackendPayload {
    kind: String,
    provider: String,
    display_name: String,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    endpoint_host: Option<String>,
    #[serde(default)]
    path_style: Option<bool>,
    #[serde(default)]
    directory: Option<String>,
    durable: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
enum HealthPayload {
    Reachable { latency_ms: u64 },
    Unreachable { reason: String },
    Unprobed,
}

#[derive(Debug, Deserialize)]
struct UsagePayload {
    artifact_count: u64,
    total_bytes: u64,
    largest_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct LimitsPayload {
    max_artifact_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct ObjectPayload {
    key: String,
    sha256: String,
    recorded_bytes: u64,
    #[serde(default)]
    stored_bytes: Option<u64>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    cache_control: Option<String>,
    #[serde(default)]
    last_modified: Option<String>,
    reconciliation: ReconciliationPayload,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
enum ReconciliationPayload {
    Consistent,
    Divergent { detail: String },
    Missing,
}

/// Pure projections from the wire shape onto what the template renders.
fn to_view(payload: StatusPayload) -> StorageView {
    StorageView {
        provider_label: payload.backend.display_name,
        provider_slug: payload.backend.provider,
        kind: payload.backend.kind,
        durable: payload.backend.durable,
        bucket: payload.backend.bucket,
        region: payload.backend.region,
        endpoint_host: payload.backend.endpoint_host,
        path_style: payload.backend.path_style,
        directory: payload.backend.directory,
        health: match payload.health {
            HealthPayload::Reachable { latency_ms } => Health::Reachable { latency_ms },
            HealthPayload::Unreachable { reason } => Health::Unreachable { reason },
            HealthPayload::Unprobed => Health::Unprobed,
        },
        artifact_count: payload.usage.artifact_count,
        total_bytes: payload.usage.total_bytes,
        largest_bytes: payload.usage.largest_bytes,
        max_artifact_bytes: payload.limits.max_artifact_bytes,
        observed_at: payload.observed_at,
    }
}

fn to_object_view(payload: ObjectPayload) -> ObjectView {
    ObjectView {
        key: payload.key,
        sha256: payload.sha256,
        recorded_bytes: payload.recorded_bytes,
        stored_bytes: payload.stored_bytes,
        content_type: payload.content_type,
        cache_control: payload.cache_control,
        last_modified: payload.last_modified,
        verdict: match payload.reconciliation {
            ReconciliationPayload::Consistent => ObjectVerdict::Consistent,
            ReconciliationPayload::Divergent { detail } => ObjectVerdict::Divergent { detail },
            ReconciliationPayload::Missing => ObjectVerdict::Missing,
        },
    }
}

/// A digest is checked here, before it can become a request path.
///
/// The API validates it too. Doing it in the browser tier as well means an
/// obvious typo is answered instantly instead of costing a delegated token
/// exchange and a round trip, and the console never asks the API about a
/// coordinate that cannot exist.
#[must_use]
fn is_artifact_digest(candidate: &str) -> bool {
    candidate.len() == 64
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Deserialize)]
pub struct StorageQuery {
    #[serde(default)]
    artifact: Option<String>,
}

pub async fn console(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(query): Query<StorageQuery>,
) -> Response {
    let viewer = session::resolve(&state, &headers).await;
    if !viewer.is_signed_in() {
        // The report names a bucket and an endpoint host — operator
        // configuration rather than public registry data.
        return axum::response::Redirect::to("/auth/sign-in?return_to=%2Fconsole%2Fstorage")
            .into_response();
    }

    let base = api_base(&state);
    let status_url = match reqwest::Url::parse(&format!("{base}/v1/storage/status")) {
        Ok(url) => url,
        Err(error) => {
            return render_unavailable(&state, &viewer, &format!("invalid API base: {error}"));
        }
    };

    let (status, rotation) = match fetch_json::<StatusPayload>(&state, &headers, status_url).await {
        Fetched::Ok(payload, rotation) => (payload, rotation),
        Fetched::Failed(reason, rotation) => {
            let mut response = render_unavailable(&state, &viewer, &reason);
            if let Some(rotation) = rotation {
                rotation.apply(&mut response);
            }
            return response;
        }
    };

    let view = to_view(status);
    let requested = query
        .artifact
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let object = match requested {
        None => None,
        Some(digest) if !is_artifact_digest(digest) => Some(ObjectResult::Invalid),
        Some(digest) => Some(reconcile(&state, &headers, &base, digest).await),
    };

    let mut response = Html(
        layout(
            "Storage",
            state.db.is_some(),
            &viewer,
            &PageContext::none(),
            page(&view, object.as_ref()),
        )
        .into_string(),
    )
    .into_response();
    if let Some(rotation) = rotation {
        rotation.apply(&mut response);
    }
    response
}

async fn reconcile(
    state: &Arc<WebState>,
    headers: &HeaderMap,
    base: &str,
    digest: &str,
) -> ObjectResult {
    let Ok(url) = reqwest::Url::parse(&format!("{base}/v1/storage/artifacts/{digest}")) else {
        return ObjectResult::Invalid;
    };
    match fetch_json::<ObjectPayload>(state, headers, url).await {
        Fetched::Ok(payload, _) => ObjectResult::Found(to_object_view(payload)),
        Fetched::Failed(reason, _) => {
            if reason.contains("404") {
                ObjectResult::NotPublished {
                    digest: digest.to_owned(),
                }
            } else {
                ObjectResult::BackendUnavailable { reason }
            }
        }
    }
}

enum Fetched<T> {
    Ok(T, Option<browser_auth::RotatedSession>),
    Failed(String, Option<browser_auth::RotatedSession>),
}

/// One delegated GET, decoded, with the session rotation handed back so the
/// caller can attach the refreshed cookie to whatever it renders.
async fn fetch_json<T: for<'de> Deserialize<'de>>(
    state: &Arc<WebState>,
    headers: &HeaderMap,
    url: reqwest::Url,
) -> Fetched<T> {
    let delegated =
        match browser_auth::delegated_get(state, headers, Method::GET, url, "application/json")
            .await
        {
            Ok(delegated) => delegated,
            Err(_) => return Fetched::Failed("browser session is unavailable".to_owned(), None),
        };
    let (outcome, rotation) = delegated.into_parts();
    match outcome {
        DelegatedGetOutcome::Upstream(response) => {
            let status = response.status();
            if !status.is_success() {
                return Fetched::Failed(format!("registry API returned {status}"), Some(rotation));
            }
            match response.json::<T>().await {
                Ok(payload) => Fetched::Ok(payload, Some(rotation)),
                Err(error) => Fetched::Failed(
                    format!("registry API sent an unreadable report: {error}"),
                    Some(rotation),
                ),
            }
        }
        DelegatedGetOutcome::Failed(_) => Fetched::Failed(
            "the registry API refused the delegated credential".to_owned(),
            Some(rotation),
        ),
    }
}

fn render_unavailable(state: &Arc<WebState>, viewer: &Viewer, reason: &str) -> Response {
    Html(
        layout(
            "Storage",
            state.db.is_some(),
            viewer,
            &PageContext::none(),
            unavailable(reason),
        )
        .into_string(),
    )
    .into_response()
}

const DEFAULT_API_BASE: &str = "https://api.zpkg.net";

fn api_base(state: &WebState) -> String {
    let zed_api_url = std::env::var("ZED_API_URL").ok();
    resolve_api_base(
        state
            .browser_auth
            .as_ref()
            .map(|config| config.api_url.as_str()),
        zed_api_url.as_deref(),
    )
}

/// Pure precedence rule, kept separate from the environment read so it can be
/// tested without mutating process state.
///
/// The configured browser-auth origin wins: it is the same origin the delegated
/// credential is minted for, so preferring anything else would send a token to
/// a host it was not issued for.
#[must_use]
fn resolve_api_base(browser_auth_api_url: Option<&str>, zed_api_url: Option<&str>) -> String {
    browser_auth_api_url
        .into_iter()
        .chain(zed_api_url)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or(DEFAULT_API_BASE)
        .trim_end_matches('/')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bytes `zed-api-server` publishes as its contract.
    const STATUS_CONTRACT: &str = include_str!("../../contracts/storage-status.v1.json");

    #[test]
    fn the_published_api_contract_decodes_here() {
        let payload: StatusPayload = serde_json::from_str(STATUS_CONTRACT)
            .expect("this console must decode the API's published storage contract");
        let view = to_view(payload);
        assert_eq!(view.provider_label, "Cloudflare R2");
        assert_eq!(view.provider_slug, "cloudflare-r2");
        assert!(view.durable);
        assert_eq!(view.health, Health::Reachable { latency_ms: 37 });
        assert_eq!(view.artifact_count, 1284);
        assert_eq!(view.max_artifact_bytes, 104_857_600);
    }

    #[test]
    fn an_added_api_field_does_not_break_the_console() {
        // Forward compatibility is deliberate: the API can report more without
        // a coordinated deploy of this page.
        let mut value: serde_json::Value = serde_json::from_str(STATUS_CONTRACT).unwrap();
        value["replication"] = serde_json::json!({"state": "in-sync"});
        value["backend"]["future_field"] = serde_json::json!(true);
        let payload: Result<StatusPayload, _> = serde_json::from_value(value);
        assert!(payload.is_ok(), "{:?}", payload.err());
    }

    #[test]
    fn every_health_state_decodes() {
        let reachable: HealthPayload =
            serde_json::from_str(r#"{"state":"reachable","latency_ms":9}"#).unwrap();
        assert!(matches!(
            reachable,
            HealthPayload::Reachable { latency_ms: 9 }
        ));
        let unreachable: HealthPayload =
            serde_json::from_str(r#"{"state":"unreachable","reason":"refused"}"#).unwrap();
        assert!(matches!(unreachable, HealthPayload::Unreachable { .. }));
        let unprobed: HealthPayload = serde_json::from_str(r#"{"state":"unprobed"}"#).unwrap();
        assert!(matches!(unprobed, HealthPayload::Unprobed));
    }

    #[test]
    fn every_reconciliation_state_decodes() {
        for (json, expected_missing) in [
            (r#"{"state":"consistent"}"#, false),
            (r#"{"state":"divergent","detail":"size"}"#, false),
            (r#"{"state":"missing"}"#, true),
        ] {
            let parsed: ReconciliationPayload = serde_json::from_str(json).unwrap();
            assert_eq!(
                matches!(parsed, ReconciliationPayload::Missing),
                expected_missing,
                "{json}"
            );
        }
    }

    #[test]
    fn only_a_real_digest_reaches_the_api() {
        assert!(is_artifact_digest(&"a".repeat(64)));
        assert!(is_artifact_digest(&"0123456789abcdef".repeat(4)));
        for rejected in [
            "".to_owned(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            format!("../{}", "a".repeat(61)),
        ] {
            assert!(
                !is_artifact_digest(&rejected),
                "{rejected} must be rejected"
            );
        }
    }

    #[test]
    fn the_api_base_never_ends_in_a_slash() {
        // A trailing slash would build `//v1/storage/status`, which some
        // gateways route differently than the path the API actually serves.
        assert_eq!(
            resolve_api_base(Some("https://api.internal/"), None),
            "https://api.internal"
        );
        assert_eq!(
            resolve_api_base(None, Some("  https://api.example.test/  ")),
            "https://api.example.test"
        );
    }

    #[test]
    fn the_delegated_origin_outranks_the_environment() {
        // The token is minted for the browser-auth origin; sending it anywhere
        // else would hand a credential to a host it was not issued for.
        assert_eq!(
            resolve_api_base(Some("https://api.internal"), Some("https://elsewhere.test")),
            "https://api.internal"
        );
        assert_eq!(
            resolve_api_base(Some("   "), Some("https://elsewhere.test")),
            "https://elsewhere.test"
        );
        assert_eq!(resolve_api_base(None, None), DEFAULT_API_BASE);
    }
}
