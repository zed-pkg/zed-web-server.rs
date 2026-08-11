//! Same-origin read BFF for dependency-graph documents and downloads.
//!
//! The browser never talks to the registry API directly. This route repeats
//! the package visibility check through the read-only data plane and then
//! relays the immutable graph representation from the API. It does not resolve,
//! translate, or persist a second graph authority.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use reqwest::Url;
use serde_json::json;

use crate::state::WebState;
use crate::{browser_auth, session};

const MAX_GRAPH_BYTES: usize = 32 * 1024 * 1024;
const GRAPH_DIGEST_HEADER: &str = "x-zpkg-graph-digest";
const GRAPH_AUTHORITY_HEADER: &str = "x-zpkg-graph-authoritative";
const SELECTED_VERSION_HEADER: &str = "x-zpkg-selected-version";
const DEFAULT_API_BASE: &str = "http://127.0.0.1:8080";
const PRIVATE_GRAPH_CACHE: &str = "private, no-store";
const LATEST_GRAPH_CACHE: &str = "public, max-age=60, must-revalidate";

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthorizedGraph {
    version: String,
    is_public: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphCachePolicy {
    PublicExact,
    PublicLatest,
    Private,
}

impl GraphCachePolicy {
    const fn for_package(is_public: bool, is_latest_route: bool) -> Self {
        match (is_public, is_latest_route) {
            (false, _) => Self::Private,
            (true, true) => Self::PublicLatest,
            (true, false) => Self::PublicExact,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportRoute {
    Canonical(&'static str),
    Extended(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GraphUrlError;

impl ExportRoute {
    fn parse(value: &str) -> Option<Self> {
        Some(match value.to_ascii_lowercase().as_str() {
            "json" => Self::Canonical("json"),
            "yaml" | "yml" => Self::Canonical("yaml"),
            "toml" => Self::Canonical("toml"),
            "dot" | "graphviz" => Self::Canonical("dot"),
            "mermaid" | "mmd" => Self::Canonical("mermaid"),
            "json5" => Self::Extended("json5"),
            "xml" => Self::Extended("xml"),
            "csv" => Self::Extended("csv"),
            "msgpack" | "messagepack" | "mpk" => Self::Extended("msgpack"),
            "protobuf" | "proto" | "pb" => Self::Extended("protobuf"),
            _ => return None,
        })
    }
}

pub async fn package_document(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, name, version)): Path<(String, String, String)>,
) -> Response {
    let authorized = match authorize_package(&state, &headers, &org, &name, Some(&version)).await {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let api_base = api_base(&state);
    let url = match declared_graph_url(&api_base, &org, &name, &version, Some("json")) {
        Ok(url) => url,
        Err(_) => return upstream_configuration_error(),
    };
    relay(
        &state,
        &headers,
        url,
        None,
        GraphCachePolicy::for_package(authorized.is_public, false),
    )
    .await
}

pub async fn latest_package_document(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, name)): Path<(String, String)>,
) -> Response {
    let authorized = match authorize_package(&state, &headers, &org, &name, None).await {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let version = authorized.version;
    let api_base = api_base(&state);
    let url = match declared_graph_url(&api_base, &org, &name, &version, Some("json")) {
        Ok(url) => url,
        Err(_) => return upstream_configuration_error(),
    };
    relay(
        &state,
        &headers,
        url,
        Some(&version),
        GraphCachePolicy::for_package(authorized.is_public, true),
    )
    .await
}

pub async fn package_export(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, name, version, requested_format)): Path<(String, String, String, String)>,
) -> Response {
    let authorized = match authorize_package(&state, &headers, &org, &name, Some(&version)).await {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let Some(format) = ExportRoute::parse(&requested_format) else {
        return problem(
            StatusCode::NOT_ACCEPTABLE,
            "unsupported_format",
            "That dependency-graph export format is not supported.",
        );
    };
    let api_base = api_base(&state);
    let url = match format {
        ExportRoute::Canonical(format) => {
            declared_graph_url(&api_base, &org, &name, &version, Some(format))
        }
        ExportRoute::Extended(format) => {
            extended_export_url(&api_base, &org, &name, &version, format)
        }
    };
    let url = match url {
        Ok(url) => url,
        Err(_) => return upstream_configuration_error(),
    };
    relay(
        &state,
        &headers,
        url,
        None,
        GraphCachePolicy::for_package(authorized.is_public, false),
    )
    .await
}

async fn authorize_package(
    state: &WebState,
    headers: &HeaderMap,
    org_slug: &str,
    name: &str,
    requested_version: Option<&str>,
) -> Result<AuthorizedGraph, Response> {
    let viewer = session::resolve(state, headers).await;
    let Some(db) = &state.db else {
        return Err(problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "registry_offline",
            "The registry database is unavailable.",
        ));
    };

    let found = match zed_orm_core::read::package_by_org_and_name(db, org_slug, name).await {
        Ok(found) => found,
        Err(error) => {
            tracing::warn!(%error, org = org_slug, package = name, "graph package lookup failed");
            return Err(problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "registry_unavailable",
                "Dependency graph metadata is temporarily unavailable.",
            ));
        }
    };
    let Some((package, org)) = found else {
        return Err(graph_not_found());
    };
    let is_public = package.visibility == "public";
    let mut can_read_private = viewer.can_see_private(&org.slug);
    if !is_public
        && !can_read_private
        && let (Some(user), Some(project_id)) = (viewer.user(), package.project_id)
    {
        let project_role =
            match zed_orm_core::read::project_role_for_user(db, project_id, user.id).await {
                Ok(role) => role,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        org = org_slug,
                        package = name,
                        "graph project membership lookup failed"
                    );
                    return Err(problem(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "registry_unavailable",
                        "Dependency graph metadata is temporarily unavailable.",
                    ));
                }
            };
        can_read_private = project_role.is_some();
    }
    if !is_public && !can_read_private {
        return Err(graph_not_found());
    }

    // Package authorization is sufficient for an exact immutable coordinate;
    // verify it with the exact indexed read rather than scanning the bounded
    // package-page listing, which would reject older published history.
    if let Some(requested) = requested_version {
        return match zed_orm_core::read::package_version_by_package_and_version(
            db, package.id, requested,
        )
        .await
        {
            Ok(Some(_)) => Ok(AuthorizedGraph {
                version: requested.to_owned(),
                is_public,
            }),
            Ok(None) => Err(graph_not_found()),
            Err(error) => {
                tracing::warn!(%error, org = org_slug, package = name, version = requested, "graph version lookup failed");
                Err(problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "registry_unavailable",
                    "Dependency graph metadata is temporarily unavailable.",
                ))
            }
        };
    }

    let versions = match zed_orm_core::read::versions_for_package(db, package.id).await {
        Ok(versions) => versions,
        Err(error) => {
            tracing::warn!(%error, org = org_slug, package = name, "graph version lookup failed");
            return Err(problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "registry_unavailable",
                "Dependency graph metadata is temporarily unavailable.",
            ));
        }
    };

    let exact_latest = match package.latest_version {
        Some(latest) => match zed_orm_core::read::package_version_by_package_and_version(
            db, package.id, &latest,
        )
        .await
        {
            Ok(Some(row)) if !row.yanked => Some(latest),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(%error, org = org_slug, package = name, version = latest, "latest graph version lookup failed");
                return Err(problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "registry_unavailable",
                    "Dependency graph metadata is temporarily unavailable.",
                ));
            }
        },
        None => None,
    };
    let version = exact_latest
        .or_else(|| {
            versions
                .iter()
                .find(|row| !row.yanked)
                .map(|row| row.version.clone())
        })
        .ok_or_else(graph_not_found)?;
    Ok(AuthorizedGraph { version, is_public })
}

fn api_base(state: &WebState) -> String {
    let zed_api_url = std::env::var("ZED_API_URL").ok();
    let public_registry_url = std::env::var("PUBLIC_REGISTRY_URL").ok();
    configured_api_base(
        state
            .browser_auth
            .as_ref()
            .map(|config| config.api_url.as_str()),
        zed_api_url.as_deref(),
        public_registry_url.as_deref(),
    )
}

fn configured_api_base(
    browser_auth_api_url: Option<&str>,
    zed_api_url: Option<&str>,
    public_registry_url: Option<&str>,
) -> String {
    browser_auth_api_url
        .into_iter()
        .chain(zed_api_url)
        .chain(public_registry_url)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or(DEFAULT_API_BASE)
        .trim_end_matches('/')
        .to_owned()
}

fn declared_graph_url(
    base: &str,
    org: &str,
    name: &str,
    version: &str,
    format: Option<&str>,
) -> Result<Url, GraphUrlError> {
    let mut url = base_url(base)?;
    url.path_segments_mut().map_err(|_| GraphUrlError)?.extend([
        "v1",
        "packages",
        org,
        name,
        "versions",
        version,
        "dependency-graph",
    ]);
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("view", "declared");
        if let Some(format) = format {
            query.append_pair("format", format);
        }
    }
    Ok(url)
}

fn extended_export_url(
    base: &str,
    org: &str,
    name: &str,
    version: &str,
    format: &str,
) -> Result<Url, GraphUrlError> {
    let mut url = base_url(base)?;
    url.path_segments_mut().map_err(|_| GraphUrlError)?.extend([
        "v1",
        "packages",
        org,
        name,
        "versions",
        version,
        "dependency-graph",
        "export",
        format,
    ]);
    Ok(url)
}

fn base_url(base: &str) -> Result<Url, GraphUrlError> {
    let normalized = format!("{}/", base.trim_end_matches('/'));
    let url = Url::parse(&normalized).map_err(|error| {
        tracing::error!(%error, "ZED_API_URL is not a valid absolute URL");
        GraphUrlError
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        tracing::error!(
            "ZED_API_URL must be an HTTP(S) base URL without credentials or query data"
        );
        return Err(GraphUrlError);
    }
    Ok(url)
}

async fn relay(
    state: &WebState,
    request_headers: &HeaderMap,
    url: Url,
    selected_version: Option<&str>,
    cache_policy: GraphCachePolicy,
) -> Response {
    if cache_policy == GraphCachePolicy::Private {
        let delegated = match browser_auth::delegated_get(
            state,
            request_headers,
            url,
            request_headers.get(header::IF_NONE_MATCH).cloned(),
        )
        .await
        {
            Ok(delegated) => delegated,
            Err(response) => return private_auth_error(response),
        };
        let (outcome, rotation) = delegated.into_parts();
        let mut response = match outcome {
            browser_auth::DelegatedGetOutcome::Upstream(upstream) => {
                relay_response(upstream, selected_version, cache_policy).await
            }
            browser_auth::DelegatedGetOutcome::Failed(response) => private_auth_error(response),
        };
        rotation.apply(&mut response);
        return response;
    }

    let mut request = state.http.get(url);
    if let Some(etag) = request_headers.get(header::IF_NONE_MATCH) {
        request = request.header(header::IF_NONE_MATCH, etag.clone());
    }
    match request.send().await {
        Ok(upstream) => relay_response(upstream, selected_version, cache_policy).await,
        Err(error) => {
            tracing::warn!(%error, "dependency graph API request failed");
            problem(
                StatusCode::BAD_GATEWAY,
                "graph_upstream_unavailable",
                "The dependency graph API is temporarily unavailable.",
            )
        }
    }
}

fn private_auth_error(response: Response) -> Response {
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        graph_not_found()
    } else {
        tracing::warn!(
            upstream_status = %response.status(),
            "private dependency graph delegation failed"
        );
        problem(
            StatusCode::BAD_GATEWAY,
            "graph_auth_unavailable",
            "Private dependency graph authentication is temporarily unavailable.",
        )
    }
}

async fn relay_response(
    mut upstream: reqwest::Response,
    selected_version: Option<&str>,
    cache_policy: GraphCachePolicy,
) -> Response {
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return graph_not_found();
    }
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        return graph_too_large();
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let mut response = problem(
            StatusCode::TOO_MANY_REQUESTS,
            "graph_rate_limited",
            "The dependency graph API is temporarily rate limited.",
        );
        copy_header(
            upstream.headers(),
            response.headers_mut(),
            header::RETRY_AFTER,
        );
        return response;
    }
    if status != StatusCode::OK && status != StatusCode::NOT_MODIFIED {
        tracing::warn!(upstream_status = %status, "dependency graph API rejected request");
        return problem(
            StatusCode::BAD_GATEWAY,
            "graph_upstream_error",
            "The dependency graph API could not produce that representation.",
        );
    }

    let mut response_headers = HeaderMap::new();
    copy_header(
        upstream.headers(),
        &mut response_headers,
        header::CONTENT_TYPE,
    );
    copy_header(
        upstream.headers(),
        &mut response_headers,
        header::CONTENT_DISPOSITION,
    );
    // Axum strips the downstream body for the automatically supported HEAD
    // method. Keep the upstream representation length so HEAD and 304 retain
    // useful exact metadata without inventing a second representation.
    copy_header(
        upstream.headers(),
        &mut response_headers,
        header::CONTENT_LENGTH,
    );
    apply_cache_policy(upstream.headers(), &mut response_headers, cache_policy);
    copy_header(upstream.headers(), &mut response_headers, header::ETAG);
    copy_named_header(
        upstream.headers(),
        &mut response_headers,
        GRAPH_DIGEST_HEADER,
    );
    copy_named_header(
        upstream.headers(),
        &mut response_headers,
        GRAPH_AUTHORITY_HEADER,
    );
    if let Some(version) = selected_version.and_then(|value| HeaderValue::from_str(value).ok()) {
        response_headers.insert(HeaderName::from_static(SELECTED_VERSION_HEADER), version);
    }

    if status == StatusCode::NOT_MODIFIED {
        return (status, response_headers).into_response();
    }

    if upstream
        .content_length()
        .is_some_and(|length| length > MAX_GRAPH_BYTES as u64)
    {
        return graph_too_large();
    }
    let mut body = Vec::with_capacity(
        upstream
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(MAX_GRAPH_BYTES),
    );
    loop {
        match upstream.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_GRAPH_BYTES.saturating_sub(body.len()) => {
                body.extend_from_slice(&chunk);
            }
            Ok(Some(_)) => return graph_too_large(),
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "reading dependency graph API response failed");
                return problem(
                    StatusCode::BAD_GATEWAY,
                    "graph_upstream_unavailable",
                    "The dependency graph API response was interrupted.",
                );
            }
        }
    }
    (status, response_headers, body).into_response()
}

fn apply_cache_policy(upstream: &HeaderMap, downstream: &mut HeaderMap, policy: GraphCachePolicy) {
    match policy {
        GraphCachePolicy::PublicExact => {
            copy_header(upstream, downstream, header::CACHE_CONTROL);
        }
        GraphCachePolicy::PublicLatest => {
            downstream.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(LATEST_GRAPH_CACHE),
            );
        }
        GraphCachePolicy::Private => {
            downstream.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(PRIVATE_GRAPH_CACHE),
            );
            downstream.insert(header::VARY, HeaderValue::from_static("Cookie"));
        }
    }
}

fn copy_header(source: &HeaderMap, target: &mut HeaderMap, name: HeaderName) {
    if let Some(value) = source.get(&name) {
        target.insert(name, value.clone());
    }
}

fn copy_named_header(source: &HeaderMap, target: &mut HeaderMap, name: &'static str) {
    let name = HeaderName::from_static(name);
    copy_header(source, target, name);
}

fn graph_not_found() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "not_found",
        "That dependency graph is not visible or does not exist.",
    )
}

fn graph_too_large() -> Response {
    problem(
        StatusCode::PAYLOAD_TOO_LARGE,
        "graph_too_large",
        "The dependency graph exceeds the browser workspace limit.",
    )
}

fn upstream_configuration_error() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "graph_upstream_unconfigured",
        "The dependency graph API is not configured.",
    )
}

fn problem(status: StatusCode, code: &str, message: &str) -> Response {
    let mut response = (status, Json(json!({ "code": code, "message": message }))).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_aliases_are_explicit_and_bounded() {
        assert_eq!(
            ExportRoute::parse("yml"),
            Some(ExportRoute::Canonical("yaml"))
        );
        assert_eq!(
            ExportRoute::parse("messagepack"),
            Some(ExportRoute::Extended("msgpack"))
        );
        assert_eq!(
            ExportRoute::parse("pb"),
            Some(ExportRoute::Extended("protobuf"))
        );
        assert!(ExportRoute::parse("pickle").is_none());
    }

    #[test]
    fn graph_api_origin_works_without_shared_auth() {
        assert_eq!(
            configured_api_base(None, None, Some(" http://127.0.0.1:49152/ ")),
            "http://127.0.0.1:49152"
        );
        assert_eq!(
            configured_api_base(
                None,
                Some("https://api.internal.example/"),
                Some("https://public.example/")
            ),
            "https://api.internal.example"
        );
        assert_eq!(
            configured_api_base(
                Some("https://delegated.internal/"),
                Some("https://api.internal.example/"),
                Some("https://public.example/")
            ),
            "https://delegated.internal"
        );
        assert_eq!(
            configured_api_base(None, Some("  "), None),
            DEFAULT_API_BASE
        );
    }

    #[test]
    fn graph_urls_encode_coordinates_and_keep_the_declared_selector() {
        let url = declared_graph_url(
            "https://api.zpkg.net",
            "acme tools",
            "http/client",
            "1.0.0-beta.1+build",
            Some("json"),
        )
        .unwrap();
        assert_eq!(url.scheme(), "https");
        assert!(url.as_str().contains("acme%20tools"));
        assert!(url.as_str().contains("http%2Fclient"));
        assert!(url.as_str().contains("view=declared"));
        assert!(url.as_str().contains("format=json"));
    }

    #[test]
    fn extended_exports_use_the_dedicated_route() {
        let url = extended_export_url("https://api.zpkg.net/", "acme", "http", "2.1.0", "protobuf")
            .unwrap();
        assert_eq!(
            url.path(),
            "/v1/packages/acme/http/versions/2.1.0/dependency-graph/export/protobuf"
        );
        assert!(url.query().is_none());
    }

    #[test]
    fn graph_api_base_rejects_non_http_and_credential_bearing_urls() {
        for invalid in [
            "file:///tmp/registry",
            "https://user:secret@api.zpkg.net",
            "https://api.zpkg.net?token=secret",
            "https://api.zpkg.net#fragment",
        ] {
            assert!(base_url(invalid).is_err(), "{invalid}");
        }
        assert!(base_url("https://api.zpkg.net/internal/v1").is_ok());
    }

    #[test]
    fn authorization_sensitive_cache_policies_override_public_upstream_headers() {
        let mut upstream = HeaderMap::new();
        upstream.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );

        let mut private = HeaderMap::new();
        apply_cache_policy(&upstream, &mut private, GraphCachePolicy::Private);
        assert_eq!(private[header::CACHE_CONTROL], PRIVATE_GRAPH_CACHE);
        assert_eq!(private[header::VARY], "Cookie");

        let mut latest = HeaderMap::new();
        apply_cache_policy(&upstream, &mut latest, GraphCachePolicy::PublicLatest);
        assert_eq!(latest[header::CACHE_CONTROL], LATEST_GRAPH_CACHE);

        let mut exact = HeaderMap::new();
        apply_cache_policy(&upstream, &mut exact, GraphCachePolicy::PublicExact);
        assert_eq!(
            exact[header::CACHE_CONTROL],
            upstream[header::CACHE_CONTROL]
        );
    }

    #[test]
    fn graph_problem_responses_are_never_shared_cacheable() {
        let response = graph_not_found();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[test]
    fn representation_length_is_allowlisted_with_other_exact_metadata() {
        let mut upstream = HeaderMap::new();
        upstream.insert(header::CONTENT_LENGTH, HeaderValue::from_static("1234"));
        let mut downstream = HeaderMap::new();
        copy_header(&upstream, &mut downstream, header::CONTENT_LENGTH);
        assert_eq!(downstream[header::CONTENT_LENGTH], "1234");
    }
}
