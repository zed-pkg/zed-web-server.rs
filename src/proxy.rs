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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::{any, get};
    use tower::util::ServiceExt;

    /// Stub upstream: echoes the request line it saw, plus fixed endpoints for
    /// header-passthrough cases.
    fn stub() -> Router {
        async fn echo(req: Request) -> String {
            let method = req.method().clone();
            let uri = req.uri().clone();
            let body = axum::body::to_bytes(req.into_body(), usize::MAX)
                .await
                .unwrap();
            format!("{method} {uri} body={}", String::from_utf8_lossy(&body))
        }
        async fn cookies() -> Response {
            let mut response = Response::new(Body::empty());
            response.headers_mut().append(
                header::SET_COOKIE,
                HeaderValue::from_static("session=abc; Path=/; HttpOnly"),
            );
            response.headers_mut().append(
                header::SET_COOKIE,
                HeaderValue::from_static("csrf=xyz; Path=/"),
            );
            response
        }
        async fn csp() -> Response {
            let mut response = Response::new(Body::from("auth page"));
            response.headers_mut().insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'"),
            );
            response
        }
        async fn redirect() -> Response {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::FOUND;
            response.headers_mut().insert(
                header::LOCATION,
                HeaderValue::from_static("https://example.com/next?x=1"),
            );
            response
        }
        Router::new()
            .route("/cookies", get(cookies))
            .route("/csp", get(csp))
            .route("/redirect", get(redirect))
            .fallback(any(echo))
    }

    async fn spawn_upstream() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, stub()).await.unwrap() });
        format!("http://{addr}")
    }

    fn app(shared_auth_url: Option<String>) -> Router {
        crate::routes::router(Arc::new(WebState {
            db: None,
            registry_url: "https://registry.zpkg.net".into(),
            shared_auth_url,
            session_path: "/auth/browser/session".into(),
            browser_auth: None,
            http: client(),
        }))
    }

    async fn send(app: Router, request: axum::http::Request<Body>) -> Response {
        app.oneshot(request).await.unwrap()
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    fn get_request(uri: &str) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn strips_prefix_at_the_root() {
        let upstream = spawn_upstream().await;
        for uri in ["/shared-auth", "/shared-auth/"] {
            let response = send(app(Some(upstream.clone())), get_request(uri)).await;
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            assert_eq!(body_of(response).await, "GET / body=", "{uri}");
        }
    }

    #[tokio::test]
    async fn strips_prefix_on_nested_paths_and_preserves_the_query() {
        let upstream = spawn_upstream().await;
        let response = send(
            app(Some(upstream)),
            get_request("/shared-auth/auth/exchange?code=abc&state=x%2Fy"),
        )
        .await;
        assert_eq!(
            body_of(response).await,
            "GET /auth/exchange?code=abc&state=x%2Fy body="
        );
    }

    #[tokio::test]
    async fn forwards_the_post_body() {
        let upstream = spawn_upstream().await;
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/shared-auth/auth/exchange")
            .body(Body::from("grant_type=code&code=abc"))
            .unwrap();
        let response = send(app(Some(upstream)), request).await;
        assert_eq!(
            body_of(response).await,
            "POST /auth/exchange body=grant_type=code&code=abc"
        );
    }

    #[tokio::test]
    async fn passes_multiple_set_cookie_headers_through() {
        let upstream = spawn_upstream().await;
        let response = send(app(Some(upstream)), get_request("/shared-auth/cookies")).await;
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .collect();
        assert_eq!(
            cookies,
            ["session=abc; Path=/; HttpOnly", "csrf=xyz; Path=/",]
        );
    }

    #[tokio::test]
    async fn upstream_security_headers_win_over_the_site_csp() {
        let upstream = spawn_upstream().await;
        let response = send(app(Some(upstream)), get_request("/shared-auth/csp")).await;
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'"
        );
    }

    #[tokio::test]
    async fn redirects_pass_through_with_location_untouched() {
        let upstream = spawn_upstream().await;
        let response = send(app(Some(upstream)), get_request("/shared-auth/redirect")).await;
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers()[header::LOCATION],
            "https://example.com/next?x=1"
        );
    }

    #[tokio::test]
    async fn unconfigured_upstream_yields_503() {
        let response = send(app(None), get_request("/shared-auth/auth/exchange")).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body_of(response).await,
            "{\"error\":\"shared-auth upstream not configured\"}"
        );
    }

    #[tokio::test]
    async fn unreachable_upstream_yields_502() {
        // Bind then drop so the port is known-closed.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let response = send(
            app(Some(format!("http://{addr}"))),
            get_request("/shared-auth/auth/exchange"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_of(response).await,
            "{\"error\":\"shared-auth upstream unreachable\"}"
        );
    }
}
