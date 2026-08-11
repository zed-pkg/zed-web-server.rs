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

use crate::session;
use crate::state::WebState;

const MAX_GRAPH_BYTES: usize = 32 * 1024 * 1024;
const GRAPH_DIGEST_HEADER: &str = "x-zpkg-graph-digest";
const GRAPH_AUTHORITY_HEADER: &str = "x-zpkg-graph-authoritative";
const SELECTED_VERSION_HEADER: &str = "x-zpkg-selected-version";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportRoute {
    Canonical(&'static str),
    Extended(&'static str),
}

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
    if let Err(response) = authorize_package(&state, &headers, &org, &name, Some(&version)).await {
        return response;
    }
    let url = match declared_graph_url(api_base(&state), &org, &name, &version, Some("json")) {
        Ok(url) => url,
        Err(response) => return response,
    };
    relay(&state, &headers, url, None).await
}

pub async fn latest_package_document(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, name)): Path<(String, String)>,
) -> Response {
    let version = match authorize_package(&state, &headers, &org, &name, None).await {
        Ok(version) => version,
        Err(response) => return response,
    };
    let url = match declared_graph_url(api_base(&state), &org, &name, &version, Some("json")) {
        Ok(url) => url,
        Err(response) => return response,
    };
    relay(&state, &headers, url, Some(&version)).await
}

pub async fn package_export(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org, name, version, requested_format)): Path<(String, String, String, String)>,
) -> Response {
    if let Err(response) = authorize_package(&state, &headers, &org, &name, Some(&version)).await {
        return response;
    }
    let Some(format) = ExportRoute::parse(&requested_format) else {
        return problem(
            StatusCode::NOT_ACCEPTABLE,
            "unsupported_format",
            "That dependency-graph export format is not supported.",
        );
    };
    let url = match format {
        ExportRoute::Canonical(format) => {
            declared_graph_url(api_base(&state), &org, &name, &version, Some(format))
        }
        ExportRoute::Extended(format) => {
            extended_export_url(api_base(&state), &org, &name, &version, format)
        }
    };
    let url = match url {
        Ok(url) => url,
        Err(response) => return response,
    };
    relay(&state, &headers, url, None).await
}

async fn authorize_package(
    state: &WebState,
    headers: &HeaderMap,
    org_slug: &str,
    name: &str,
    requested_version: Option<&str>,
) -> Result<String, Response> {
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
    if package.visibility != "public" && !viewer.can_see_private(&org.slug) {
        return Err(graph_not_found());
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

    if let Some(requested) = requested_version {
        return versions
            .iter()
            .any(|row| row.version == requested)
            .then_some(requested.to_owned())
            .ok_or_else(graph_not_found);
    }

    package
        .latest_version
        .filter(|latest| {
            versions
                .iter()
                .any(|row| row.version.as_str() == latest.as_str())
        })
        .or_else(|| {
            versions
                .iter()
                .find(|row| !row.yanked)
                .map(|row| row.version.clone())
        })
        .ok_or_else(graph_not_found)
}

fn api_base(state: &WebState) -> &str {
    state
        .browser_auth
        .as_ref()
        .map(|config| config.api_url.as_str())
        .unwrap_or("http://127.0.0.1:8080")
}

fn declared_graph_url(
    base: &str,
    org: &str,
    name: &str,
    version: &str,
    format: Option<&str>,
) -> Result<Url, Response> {
    let mut url = base_url(base)?;
    url.path_segments_mut()
        .map_err(|_| upstream_configuration_error())?
        .extend([
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
) -> Result<Url, Response> {
    let mut url = base_url(base)?;
    url.path_segments_mut()
        .map_err(|_| upstream_configuration_error())?
        .extend([
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

fn base_url(base: &str) -> Result<Url, Response> {
    let normalized = format!("{}/", base.trim_end_matches('/'));
    Url::parse(&normalized).map_err(|error| {
        tracing::error!(%error, "ZED_API_URL is not a valid absolute URL");
        upstream_configuration_error()
    })
}

async fn relay(
    state: &WebState,
    request_headers: &HeaderMap,
    url: Url,
    selected_version: Option<&str>,
) -> Response {
    let mut request = state.http.get(url);
    if let Some(etag) = request_headers.get(header::IF_NONE_MATCH) {
        request = request.header(header::IF_NONE_MATCH, etag.clone());
    }
    let upstream = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "dependency graph API request failed");
            return problem(
                StatusCode::BAD_GATEWAY,
                "graph_upstream_unavailable",
                "The dependency graph API is temporarily unavailable.",
            );
        }
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status == StatusCode::NOT_FOUND {
        return graph_not_found();
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
    copy_header(
        upstream.headers(),
        &mut response_headers,
        header::CACHE_CONTROL,
    );
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

    let body = match upstream.bytes().await {
        Ok(bytes) if bytes.len() <= MAX_GRAPH_BYTES => bytes,
        Ok(_) => {
            return problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "graph_too_large",
                "The dependency graph exceeds the browser workspace limit.",
            );
        }
        Err(error) => {
            tracing::warn!(%error, "reading dependency graph API response failed");
            return problem(
                StatusCode::BAD_GATEWAY,
                "graph_upstream_unavailable",
                "The dependency graph API response was interrupted.",
            );
        }
    };
    (status, response_headers, body).into_response()
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

fn upstream_configuration_error() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "graph_upstream_unconfigured",
        "The dependency graph API is not configured.",
    )
}

fn problem(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "code": code, "message": message }))).into_response()
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
}
