//! Reverse proxy for the shared-auth service under `/shared-auth`.
//!
//! Mirrors the canonical nginx gateway convention (`/shared-auth(/|$)(.*)` →
//! `/$2`): the prefix is stripped and everything else — method, query, body,
//! headers, status, redirects — passes through verbatim. The auth server owns
//! prefix-aware link generation (`AUTH_BROWSER_PUBLIC_PREFIX`), so neither
//! response bodies nor Location headers are rewritten here.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::state::WebState;

/// One client for the whole process: pooled connections, and a redirect policy
/// of NONE so auth-flow 3xx responses reach the browser untouched.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static reqwest client configuration must build")
}

/// Connection-scoped headers (RFC 9110 §7.6.1, plus Host) that describe the
/// client↔gateway hop and would corrupt the gateway↔upstream hop if copied.
fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn error_json(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
}

/// Request headers for the upstream hop: end-to-end headers copied with
/// multi-values intact (cookies!), plus the X-Forwarded-* trio. Host and
/// Content-Length are owned by the client for the new hop.
fn upstream_headers(req: &Request, client_addr: Option<SocketAddr>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in req.headers() {
        if !is_hop_by_hop(name) && *name != header::CONTENT_LENGTH {
            headers.append(name.clone(), value.clone());
        }
    }
    // An outer ingress that already set Proto/Host knows better; only fill
    // the gaps. X-Forwarded-For is append-only by definition.
    if let Some(host) = req.headers().get(header::HOST).cloned() {
        headers
            .entry(HeaderName::from_static("x-forwarded-host"))
            .or_insert(host);
    }
    headers
        .entry(HeaderName::from_static("x-forwarded-proto"))
        .or_insert(HeaderValue::from_static("http"));
    if let Some(addr) = client_addr
        && let Ok(value) = HeaderValue::from_str(&addr.ip().to_string())
    {
        headers.append(HeaderName::from_static("x-forwarded-for"), value);
    }
    headers
}

pub async fn forward(State(state): State<Arc<WebState>>, req: Request) -> Response {
    let Some(base) = &state.shared_auth_url else {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "shared-auth upstream not configured",
        );
    };

    // `/shared-auth/auth/exchange` → `/auth/exchange`; `/shared-auth[/]` → `/`.
    let path = req.uri().path();
    let mut rest = path.strip_prefix("/shared-auth").unwrap_or(path);
    if rest.is_empty() {
        rest = "/";
    }
    let target = match req.uri().query() {
        Some(query) => format!("{base}{rest}?{query}"),
        None => format!("{base}{rest}"),
    };

    let client_addr = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    let method = req.method().clone();
    let headers = upstream_headers(&req, client_addr);

    // Buffered rather than streamed so the client owns Content-Length framing;
    // auth exchanges are small and the global 10s timeout still applies.
    let body = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "request body unreadable"),
    };

    let upstream = match state
        .http
        .request(method, &target)
        .headers(headers)
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, target, "shared-auth upstream request failed");
            return error_json(StatusCode::BAD_GATEWAY, "shared-auth upstream unreachable");
        }
    };

    let status = upstream.status();
    let mut headers = HeaderMap::new();
    for (name, value) in upstream.headers() {
        if !is_hop_by_hop(name) {
            headers.append(name.clone(), value.clone());
        }
    }
    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}
