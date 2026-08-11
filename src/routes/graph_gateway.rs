//! Same-origin, read-only gateway for immutable dependency-graph representations.
//!
//! The API server remains the serializer and cache validator. This module only
//! constructs the one declared-graph route and copies a fixed request/response
//! header allowlist; it is deliberately not a generic reverse proxy.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::state::WebState;

const DIGEST_HEADER: &str = "x-zpkg-graph-digest";
const MAX_COORDINATE_LENGTH: usize = 128;

#[derive(Debug, Default, Deserialize)]
pub struct GraphQuery {
    format: Option<String>,
}

fn error_json(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
}

fn supported_format(value: &str) -> bool {
    matches!(value, "json" | "yaml" | "toml" | "dot" | "mermaid")
}

fn valid_coordinate(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_COORDINATE_LENGTH
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+' | b'@')
        })
}

fn upstream_base(state: &WebState) -> Option<String> {
    let configured = state.registry_url.trim().trim_end_matches('/');
    if !configured.is_empty() {
        return Some(configured.to_owned());
    }
    if let Some(config) = &state.browser_auth {
        let configured = config.api_url.trim().trim_end_matches('/');
        if !configured.is_empty() {
            return Some(configured.to_owned());
        }
    }
    std::env::var("ZED_API_URL").ok().and_then(|value| {
        let value = value.trim().trim_end_matches('/');
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn declared_graph_url(
    base: &str,
    org: &str,
    name: &str,
    version: &str,
    format: Option<&str>,
) -> Result<reqwest::Url, ()> {
    let mut url = reqwest::Url::parse(base).map_err(|_| ())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    {
        let mut segments = url.path_segments_mut().map_err(|_| ())?;
        segments.pop_if_empty();
        segments.extend([
            "v1",
            "packages",
            org,
            name,
            "versions",
            version,
            "dependency-graph",
        ]);
    }
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("view", "declared");
        if let Some(format) = format {
            query.append_pair("format", format);
        }
    }
    Ok(url)
}

fn forwarded_request_headers(headers: &HeaderMap) -> HeaderMap {
    let mut allowed = HeaderMap::new();
    for name in [header::ACCEPT, header::IF_NONE_MATCH] {
        for value in headers.get_all(&name).iter() {
            allowed.append(name.clone(), value.clone());
        }
    }
    allowed
}

fn copy_header(source: &HeaderMap, destination: &mut HeaderMap, name: HeaderName) {
    for value in source.get_all(&name).iter() {
        destination.append(name.clone(), value.clone());
    }
}

pub async fn declared(
    State(state): State<Arc<WebState>>,
    Path((org, name, version)): Path<(String, String, String)>,
    Query(query): Query<GraphQuery>,
    headers: HeaderMap,
) -> Response {
    if [&org, &name, &version]
        .into_iter()
        .any(|value| !valid_coordinate(value))
    {
        return error_json(StatusCode::BAD_REQUEST, "invalid package coordinate");
    }
    if let Some(format) = query.format.as_deref()
        && !supported_format(format)
    {
        return error_json(StatusCode::NOT_ACCEPTABLE, "unsupported graph format");
    }
    let Some(base) = upstream_base(&state) else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "graph API upstream not configured");
    };
    let Ok(target) = declared_graph_url(&base, &org, &name, &version, query.format.as_deref())
    else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "graph API upstream is invalid");
    };

    let upstream = match state
        .http
        .get(target.clone())
        .headers(forwarded_request_headers(&headers))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, %target, "dependency graph upstream request failed");
            return error_json(StatusCode::BAD_GATEWAY, "graph API upstream unreachable");
        }
    };

    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    *response.status_mut() = status;
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_DISPOSITION,
        header::ETAG,
        header::CACHE_CONTROL,
        HeaderName::from_static(DIGEST_HEADER),
    ] {
        copy_header(&upstream_headers, response.headers_mut(), name);
    }
    response
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use axum::Router;
    use axum::routing::get;
    use tower::util::ServiceExt;

    async fn upstream_graph(
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> Response {
        if headers
            .get(header::IF_NONE_MATCH)
            .is_some_and(|value| value == "\"graph-etag\"")
        {
            let mut response = StatusCode::NOT_MODIFIED.into_response();
            response
                .headers_mut()
                .insert(header::ETAG, "\"graph-etag\"".parse().unwrap());
            response.headers_mut().insert(
                HeaderName::from_static(DIGEST_HEADER),
                "sha256:semantic".parse().unwrap(),
            );
            return response;
        }

        let body = format!(
            "view={} format={} accept={} auth={}",
            query.get("view").map(String::as_str).unwrap_or(""),
            query.get("format").map(String::as_str).unwrap_or("accept"),
            headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .unwrap_or(""),
            headers.contains_key(header::AUTHORIZATION),
        );
        let mut response = Response::new(Body::from(body));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            "text/vnd.graphviz; charset=utf-8".parse().unwrap(),
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            "attachment; filename=graph.dot".parse().unwrap(),
        );
        response
            .headers_mut()
            .insert(header::ETAG, "\"graph-etag\"".parse().unwrap());
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            "public, max-age=31536000, immutable".parse().unwrap(),
        );
        response.headers_mut().insert(
            HeaderName::from_static(DIGEST_HEADER),
            "sha256:semantic".parse().unwrap(),
        );
        response.headers_mut().insert(
            HeaderName::from_static("x-upstream-secret"),
            "must-not-pass".parse().unwrap(),
        );
        response.headers_mut().append(
            header::SET_COOKIE,
            "upstream=forbidden; HttpOnly".parse().unwrap(),
        );
        response
    }

    async fn spawn_upstream() -> String {
        let app = Router::new().route(
            "/v1/packages/{org}/{name}/versions/{version}/dependency-graph",
            get(upstream_graph),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }

    fn app(registry_url: String) -> Router {
        crate::routes::router(Arc::new(WebState {
            db: None,
            registry_url,
            shared_auth_url: None,
            session_path: "/auth/browser/session".into(),
            browser_auth: None,
            http: crate::proxy::client(),
        }))
    }

    async fn body(response: Response) -> String {
        String::from_utf8_lossy(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .to_string()
    }

    #[tokio::test]
    async fn preserves_exact_representation_and_only_allowlisted_headers() {
        let upstream = spawn_upstream().await;
        let request = axum::http::Request::builder()
            .uri("/api/v1/packages/acme/widget/versions/1.2.3/dependency-graph?format=dot")
            .header(header::ACCEPT, "text/vnd.graphviz")
            .header(header::AUTHORIZATION, "Bearer must-not-pass")
            .body(Body::empty())
            .unwrap();
        let response = app(upstream).oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/vnd.graphviz; charset=utf-8");
        assert_eq!(response.headers()[header::CONTENT_DISPOSITION], "attachment; filename=graph.dot");
        assert_eq!(response.headers()[header::ETAG], "\"graph-etag\"");
        assert_eq!(response.headers()[DIGEST_HEADER], "sha256:semantic");
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        assert!(response.headers().get("x-upstream-secret").is_none());
        assert_eq!(
            body(response).await,
            "view=declared format=dot accept=text/vnd.graphviz auth=false"
        );
    }

    #[tokio::test]
    async fn forwards_conditional_requests_and_preserves_not_modified() {
        let upstream = spawn_upstream().await;
        let request = axum::http::Request::builder()
            .uri("/api/v1/packages/acme/widget/versions/1.2.3/dependency-graph")
            .header(header::IF_NONE_MATCH, "\"graph-etag\"")
            .body(Body::empty())
            .unwrap();
        let response = app(upstream).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], "\"graph-etag\"");
        assert_eq!(body(response).await, "");
    }

    #[tokio::test]
    async fn rejects_unknown_formats_and_invalid_coordinates_before_proxying() {
        let unreachable = "http://127.0.0.1:1".to_owned();
        let unsupported = axum::http::Request::builder()
            .uri("/api/v1/packages/acme/widget/versions/1.2.3/dependency-graph?format=xml")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app(unreachable.clone())
                .oneshot(unsupported)
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_ACCEPTABLE
        );

        let invalid = axum::http::Request::builder()
            .uri("/api/v1/packages/bad%20org/widget/versions/1.2.3/dependency-graph")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app(unreachable).oneshot(invalid).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
}
