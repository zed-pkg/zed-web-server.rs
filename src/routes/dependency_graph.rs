//! Same-origin read BFF for dependency-graph documents and downloads.
//!
//! The browser never talks to the registry API directly. This route repeats
//! the package visibility check through the read-only data plane and then
//! relays the immutable graph representation from the API. It does not resolve,
//! translate, or persist a second graph authority.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use reqwest::Url;
use serde_json::json;
use zed_interfaces::{
    DEPENDENCY_GRAPH_AUTHORITATIVE_HEADER, DEPENDENCY_GRAPH_DEFAULT_MAX_ENCODED_BYTES,
    DEPENDENCY_GRAPH_DIGEST_HEADER, DependencyGraphExportFormat, DependencyGraphFormat,
};

use crate::state::WebState;
use crate::{browser_auth, session};

const MAX_GRAPH_BYTES: usize = DEPENDENCY_GRAPH_DEFAULT_MAX_ENCODED_BYTES as usize;
const MAX_COORDINATE_LENGTH: usize = 128;
const SELECTED_VERSION_HEADER: &str = "x-zpkg-selected-version";
const DEFAULT_API_BASE: &str = "http://127.0.0.1:8080";
const PUBLIC_EXACT_GRAPH_CACHE: &str = "public, max-age=31536000, immutable";
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
    Canonical(DependencyGraphFormat),
    Extended(DependencyGraphExportFormat),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GraphUrlError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RepresentationContract {
    media_type: &'static str,
    authoritative: Option<bool>,
}

impl RepresentationContract {
    const fn json() -> Self {
        Self {
            media_type: DependencyGraphFormat::Json.media_type(),
            authoritative: Some(DependencyGraphFormat::Json.is_authoritative()),
        }
    }

    const fn for_export(format: ExportRoute) -> Self {
        Self {
            media_type: format.media_type(),
            authoritative: format.authoritative_header(),
        }
    }
}

impl ExportRoute {
    fn parse(value: &str) -> Option<Self> {
        DependencyGraphFormat::parse_name(value)
            .map(Self::Canonical)
            .or_else(|| DependencyGraphExportFormat::parse_name(value).map(Self::Extended))
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Canonical(format) => format.name(),
            Self::Extended(format) => format.name(),
        }
    }

    const fn media_type(self) -> &'static str {
        match self {
            Self::Canonical(format) => format.media_type(),
            Self::Extended(format) => format.media_type(),
        }
    }

    const fn authoritative_header(self) -> Option<bool> {
        match self {
            Self::Canonical(format) => Some(format.is_authoritative()),
            Self::Extended(format) => Some(format.is_authoritative()),
        }
    }
}

pub async fn package_document(
    State(state): State<Arc<WebState>>,
    method: Method,
    headers: HeaderMap,
    Path((org, name, version)): Path<(String, String, String)>,
) -> Response {
    if !valid_package_coordinate(&org, &name, Some(&version)) {
        return invalid_coordinate();
    }
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
        method,
        &headers,
        url,
        None,
        GraphCachePolicy::for_package(authorized.is_public, false),
        RepresentationContract::json(),
    )
    .await
}

pub async fn latest_package_document(
    State(state): State<Arc<WebState>>,
    method: Method,
    headers: HeaderMap,
    Path((org, name)): Path<(String, String)>,
) -> Response {
    if !valid_package_coordinate(&org, &name, None) {
        return invalid_coordinate();
    }
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
        method,
        &headers,
        url,
        Some(&version),
        GraphCachePolicy::for_package(authorized.is_public, true),
        RepresentationContract::json(),
    )
    .await
}

pub async fn package_export(
    State(state): State<Arc<WebState>>,
    method: Method,
    headers: HeaderMap,
    Path((org, name, version, requested_format)): Path<(String, String, String, String)>,
) -> Response {
    if !valid_package_coordinate(&org, &name, Some(&version)) {
        return invalid_coordinate();
    }
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
        ExportRoute::Canonical(_) => {
            declared_graph_url(&api_base, &org, &name, &version, Some(format.name()))
        }
        ExportRoute::Extended(_) => {
            extended_export_url(&api_base, &org, &name, &version, format.name())
        }
    };
    let url = match url {
        Ok(url) => url,
        Err(_) => return upstream_configuration_error(),
    };
    relay(
        &state,
        method,
        &headers,
        url,
        None,
        GraphCachePolicy::for_package(authorized.is_public, false),
        RepresentationContract::for_export(format),
    )
    .await
}

fn valid_package_coordinate(org: &str, name: &str, version: Option<&str>) -> bool {
    [Some(org), Some(name), version]
        .into_iter()
        .flatten()
        .all(valid_coordinate_component)
}

fn valid_coordinate_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_COORDINATE_LENGTH
        && !matches!(value, "." | "..")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+' | b'@')
        })
}

fn invalid_coordinate() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "invalid_coordinate",
        "The package coordinate is not valid.",
    )
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
    let mut can_read_private = false;
    if !is_public {
        can_read_private = match session::exact_org_role(db, &viewer, org.id).await {
            Ok(role) => role.is_some(),
            Err(error) => {
                tracing::warn!(
                    %error,
                    org = org_slug,
                    package = name,
                    "graph organization membership lookup failed"
                );
                return Err(problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "registry_unavailable",
                    "Dependency graph metadata is temporarily unavailable.",
                ));
            }
        };
        if !can_read_private && let Some(project_id) = package.project_id {
            can_read_private = match session::exact_project_role(db, &viewer, project_id).await {
                Ok(role) => role.is_some(),
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
        }
    }
    if !is_public && !can_read_private {
        return Err(graph_not_found());
    }

    // Resolve an exact immutable coordinate through the dedicated key lookup.
    // Never scan the page-oriented version listing here: older history remains
    // addressable after a package has more than one page of releases.
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
                tracing::warn!(
                    %error,
                    org = org_slug,
                    package = name,
                    version = requested,
                    "exact graph version lookup failed"
                );
                Err(problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "registry_unavailable",
                    "Dependency graph metadata is temporarily unavailable.",
                ))
            }
        };
    }

    let Some(latest) = package.latest_version else {
        return Err(graph_not_found());
    };
    let version = match zed_orm_core::read::package_version_by_package_and_version(
        db, package.id, &latest,
    )
    .await
    {
        Ok(Some(version)) if !version.yanked => version.version,
        Ok(_) => return Err(graph_not_found()),
        Err(error) => {
            tracing::warn!(%error, org = org_slug, package = name, "graph version lookup failed");
            return Err(problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "registry_unavailable",
                "Dependency graph metadata is temporarily unavailable.",
            ));
        }
    };
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
    method: Method,
    request_headers: &HeaderMap,
    url: Url,
    selected_version: Option<&str>,
    cache_policy: GraphCachePolicy,
    contract: RepresentationContract,
) -> Response {
    if cache_policy == GraphCachePolicy::Private {
        let delegated = match browser_auth::delegated_get(
            state,
            request_headers,
            method.clone(),
            url,
            contract.media_type,
        )
        .await
        {
            Ok(delegated) => delegated,
            Err(response) => return private_auth_error(response),
        };
        let (outcome, rotation) = delegated.into_parts();
        let mut response = match outcome {
            browser_auth::DelegatedGetOutcome::Upstream(upstream) => {
                relay_response(
                    upstream,
                    method == Method::HEAD,
                    selected_version,
                    cache_policy,
                    contract,
                )
                .await
            }
            browser_auth::DelegatedGetOutcome::Failed(response) => private_auth_error(response),
        };
        rotation.apply(&mut response);
        return response;
    }

    let mut request = state
        .http
        .request(method.clone(), url)
        .header(header::ACCEPT, contract.media_type)
        .header(header::ACCEPT_ENCODING, "identity");
    for etag in request_headers.get_all(header::IF_NONE_MATCH) {
        request = request.header(header::IF_NONE_MATCH, etag);
    }
    match request.send().await {
        Ok(upstream) => {
            relay_response(
                upstream,
                method == Method::HEAD,
                selected_version,
                cache_policy,
                contract,
            )
            .await
        }
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
    is_head: bool,
    selected_version: Option<&str>,
    cache_policy: GraphCachePolicy,
    contract: RepresentationContract,
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
    if status == StatusCode::UNPROCESSABLE_ENTITY {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "graph_unprocessable",
            "That dependency graph cannot be represented within the selected format or limits.",
        );
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

    if !valid_graph_validators(upstream.headers()) {
        tracing::warn!("dependency graph API omitted required representation validators");
        return problem(
            StatusCode::BAD_GATEWAY,
            "graph_upstream_contract_error",
            "The dependency graph API returned invalid representation metadata.",
        );
    }
    if status == StatusCode::OK && !content_type_matches(upstream.headers(), contract.media_type) {
        tracing::warn!(
            expected_media_type = contract.media_type,
            "dependency graph API returned the wrong media type"
        );
        return problem(
            StatusCode::BAD_GATEWAY,
            "graph_upstream_contract_error",
            "The dependency graph API returned the wrong representation type.",
        );
    }
    if status == StatusCode::OK && !content_encoding_is_identity(upstream.headers()) {
        tracing::warn!("dependency graph API returned an unexpected content encoding");
        return problem(
            StatusCode::BAD_GATEWAY,
            "graph_upstream_contract_error",
            "The dependency graph API returned an unsupported content encoding.",
        );
    }
    if !valid_attachment_disposition(upstream.headers()) {
        tracing::warn!("dependency graph API omitted the required safe download filename");
        return problem(
            StatusCode::BAD_GATEWAY,
            "graph_upstream_contract_error",
            "The dependency graph API returned invalid download metadata.",
        );
    }
    if let Some(expected) = contract.authoritative
        && !authority_header_matches(upstream.headers(), expected)
    {
        tracing::warn!(
            expected,
            "dependency graph API returned the wrong authority marker"
        );
        return problem(
            StatusCode::BAD_GATEWAY,
            "graph_upstream_contract_error",
            "The dependency graph API returned inconsistent representation metadata.",
        );
    }
    let Some(content_length) = selected_representation_length(upstream.headers()) else {
        tracing::warn!("dependency graph API omitted the selected representation length");
        return problem(
            StatusCode::BAD_GATEWAY,
            "graph_upstream_contract_error",
            "The dependency graph API returned invalid representation metadata.",
        );
    };
    if content_length > MAX_GRAPH_BYTES as u64 {
        return graph_too_large();
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
    apply_cache_policy(upstream.headers(), &mut response_headers, cache_policy);
    copy_header(upstream.headers(), &mut response_headers, header::ETAG);
    copy_header(
        upstream.headers(),
        &mut response_headers,
        header::CONTENT_LENGTH,
    );
    copy_named_header(
        upstream.headers(),
        &mut response_headers,
        DEPENDENCY_GRAPH_DIGEST_HEADER,
    );
    copy_named_header(
        upstream.headers(),
        &mut response_headers,
        DEPENDENCY_GRAPH_AUTHORITATIVE_HEADER,
    );
    if let Some(version) = selected_version.and_then(|value| HeaderValue::from_str(value).ok()) {
        response_headers.insert(HeaderName::from_static(SELECTED_VERSION_HEADER), version);
    }

    if status == StatusCode::NOT_MODIFIED {
        return super::not_modified_with_representation_metadata(response_headers);
    }

    if is_head {
        return (status, response_headers).into_response();
    }
    let mut body = Vec::with_capacity(
        usize::try_from(content_length)
            .unwrap_or(MAX_GRAPH_BYTES)
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
    if body.len() as u64 != content_length {
        tracing::warn!(
            declared_length = content_length,
            actual_length = body.len(),
            "dependency graph API returned a body with the wrong representation length"
        );
        return problem(
            StatusCode::BAD_GATEWAY,
            "graph_upstream_contract_error",
            "The dependency graph API returned inconsistent representation metadata.",
        );
    }
    (status, response_headers, body).into_response()
}

fn apply_cache_policy(upstream: &HeaderMap, downstream: &mut HeaderMap, policy: GraphCachePolicy) {
    let upstream_forbids_storage = cache_control_has_directive(upstream, "no-store")
        || cache_control_has_directive(upstream, "private");
    match (policy, upstream_forbids_storage) {
        (GraphCachePolicy::Private, _) => {
            downstream.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(PRIVATE_GRAPH_CACHE),
            );
        }
        (_, true) => {
            // A visibility change between the database authorization read and
            // the API response must only make caching stricter.
            downstream.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        }
        (GraphCachePolicy::PublicExact, false) => {
            downstream.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(PUBLIC_EXACT_GRAPH_CACHE),
            );
        }
        (GraphCachePolicy::PublicLatest, false) => {
            downstream.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(LATEST_GRAPH_CACHE),
            );
        }
    }
    downstream.insert(
        header::VARY,
        HeaderValue::from_static(match policy {
            GraphCachePolicy::Private => "Accept, Cookie",
            GraphCachePolicy::PublicExact | GraphCachePolicy::PublicLatest => "Accept",
        }),
    );
}

fn cache_control_has_directive(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get_all(header::CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .map(|directive| {
            directive
                .split_once('=')
                .map_or(directive, |(name, _)| name)
                .trim()
        })
        .any(|directive| directive.eq_ignore_ascii_case(expected))
}

fn valid_graph_validators(headers: &HeaderMap) -> bool {
    headers.get(header::ETAG).is_some_and(is_strong_etag)
        && headers
            .get(DEPENDENCY_GRAPH_DIGEST_HEADER)
            .is_some_and(is_graph_digest)
}

fn selected_representation_length(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(header::CONTENT_LENGTH)?.to_str().ok()?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn is_strong_etag(value: &HeaderValue) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes.first() == Some(&b'"')
        && bytes.last() == Some(&b'"')
        && bytes[1..bytes.len() - 1]
            .iter()
            .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte) || *byte >= 0x80)
}

fn is_graph_digest(value: &HeaderValue) -> bool {
    value
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("sha256:"))
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn content_type_matches(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|actual| {
            media_type_essence(actual).eq_ignore_ascii_case(media_type_essence(expected))
        })
}

fn content_encoding_is_identity(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .all(|value| {
            value.to_str().is_ok_and(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .all(|encoding| encoding.eq_ignore_ascii_case("identity"))
            })
        })
}

fn valid_attachment_disposition(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("attachment; filename=\""))
        .and_then(|value| value.strip_suffix('"'))
        .is_some_and(|filename| {
            !filename.is_empty()
                && filename.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_')
                })
        })
}

fn media_type_essence(value: &str) -> &str {
    value.split(';').next().unwrap_or_default().trim()
}

fn authority_header_matches(headers: &HeaderMap, expected: bool) -> bool {
    headers
        .get(DEPENDENCY_GRAPH_AUTHORITATIVE_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == if expected { "true" } else { "false" })
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
            Some(ExportRoute::Canonical(DependencyGraphFormat::Yaml))
        );
        assert_eq!(
            ExportRoute::parse("messagepack"),
            Some(ExportRoute::Extended(
                DependencyGraphExportFormat::MessagePack
            ))
        );
        assert_eq!(
            ExportRoute::parse("pb"),
            Some(ExportRoute::Extended(DependencyGraphExportFormat::Protobuf))
        );
        assert!(ExportRoute::parse("pickle").is_none());
    }

    #[test]
    fn package_coordinates_are_ascii_bounded_and_path_safe() {
        assert!(valid_package_coordinate(
            "acme",
            "http-kit",
            Some("2.0.0-beta.1+build.7")
        ));
        assert!(!valid_package_coordinate(
            "acme tools",
            "http-kit",
            Some("2.0.0")
        ));
        assert!(!valid_package_coordinate(
            "acme",
            "http/client",
            Some("2.0.0")
        ));
        assert!(!valid_package_coordinate("acme", "..", Some("2.0.0")));
        assert!(!valid_package_coordinate(
            "a".repeat(MAX_COORDINATE_LENGTH + 1).as_str(),
            "http-kit",
            Some("2.0.0")
        ));
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
    fn authorization_sensitive_cache_policies_are_owned_by_the_bff() {
        let upstream = HeaderMap::new();
        let mut private = HeaderMap::new();
        apply_cache_policy(&upstream, &mut private, GraphCachePolicy::Private);
        assert_eq!(private[header::CACHE_CONTROL], PRIVATE_GRAPH_CACHE);
        assert_eq!(private[header::VARY], "Accept, Cookie");

        let mut latest = HeaderMap::new();
        apply_cache_policy(&upstream, &mut latest, GraphCachePolicy::PublicLatest);
        assert_eq!(latest[header::CACHE_CONTROL], LATEST_GRAPH_CACHE);
        assert_eq!(latest[header::VARY], "Accept");

        let mut exact = HeaderMap::new();
        apply_cache_policy(&upstream, &mut exact, GraphCachePolicy::PublicExact);
        assert_eq!(exact[header::CACHE_CONTROL], PUBLIC_EXACT_GRAPH_CACHE);

        let mut no_store_upstream = HeaderMap::new();
        no_store_upstream.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, NO-STORE"),
        );
        let mut guarded = HeaderMap::new();
        apply_cache_policy(
            &no_store_upstream,
            &mut guarded,
            GraphCachePolicy::PublicExact,
        );
        assert_eq!(guarded[header::CACHE_CONTROL], "no-store");

        let mut private_upstream = HeaderMap::new();
        private_upstream.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=0"),
        );
        let mut race_guarded = HeaderMap::new();
        apply_cache_policy(
            &private_upstream,
            &mut race_guarded,
            GraphCachePolicy::PublicExact,
        );
        assert_eq!(race_guarded[header::CACHE_CONTROL], "no-store");
    }

    #[test]
    fn upstream_graph_metadata_requires_strong_byte_and_semantic_validators() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ETAG, HeaderValue::from_static("\"bytes\""));
        headers.insert(
            HeaderName::from_static(DEPENDENCY_GRAPH_DIGEST_HEADER),
            HeaderValue::from_static(
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
        );
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("4096"));
        assert!(valid_graph_validators(&headers));
        assert_eq!(selected_representation_length(&headers), Some(4096));

        headers.insert(header::ETAG, HeaderValue::from_static("W/\"bytes\""));
        assert!(!valid_graph_validators(&headers));
        headers.insert(header::ETAG, HeaderValue::from_static("\"bytes\""));
        headers.insert(
            HeaderName::from_static(DEPENDENCY_GRAPH_DIGEST_HEADER),
            HeaderValue::from_static("sha256:ABCDEF"),
        );
        assert!(!valid_graph_validators(&headers));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("4KiB"));
        assert_eq!(selected_representation_length(&headers), None);
    }

    #[test]
    fn upstream_media_and_authority_markers_must_match_the_selected_export() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/csv; charset=utf-8"),
        );
        headers.insert(
            HeaderName::from_static(DEPENDENCY_GRAPH_AUTHORITATIVE_HEADER),
            HeaderValue::from_static("false"),
        );
        assert!(content_type_matches(&headers, "text/csv; charset=utf-8"));
        assert!(!content_type_matches(
            &headers,
            "application/vnd.zpkg.dependency-graph.v1+json"
        ));
        assert!(authority_header_matches(&headers, false));
        assert!(!authority_header_matches(&headers, true));
        assert_eq!(RepresentationContract::json().authoritative, Some(true));
    }

    #[test]
    fn upstream_download_metadata_is_safe_and_unencoded() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static(
                "attachment; filename=\"acme_http_1.0.0.dependency-graph.json\"",
            ),
        );
        assert!(valid_attachment_disposition(&headers));
        assert!(content_encoding_is_identity(&headers));

        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"../private.json\""),
        );
        assert!(!valid_attachment_disposition(&headers));
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert!(!content_encoding_is_identity(&headers));
    }

    #[test]
    fn graph_problem_responses_are_never_shared_cacheable() {
        let response = graph_not_found();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
