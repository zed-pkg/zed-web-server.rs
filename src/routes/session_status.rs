//! Token-blind session state for the static `zpkg.net` marketing header.
//!
//! The static site may learn only whether the product's host-only signed cookie
//! resolves to a viewer. It never receives a principal, token, session id, role,
//! or exact expiry. CORS is pinned to the one marketing origin and every response
//! is non-cacheable.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::session;
use crate::state::WebState;

const MARKETING_ORIGIN: &str = "https://zpkg.net";
const DEFAULT_DASHBOARD_URL: &str = "https://app.zpkg.net/dashboard";
const CHECK_AFTER_SECONDS: u64 = 50 * 60;

fn cors_header(name: &'static str) -> HeaderName {
    HeaderName::from_static(name)
}

fn origin_is_allowed(headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin == MARKETING_ORIGIN)
}

fn add_non_cacheable_headers(response: &mut Response, preflight: bool) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::VARY,
        HeaderValue::from_static(if preflight {
            "Origin, Access-Control-Request-Method, Access-Control-Request-Headers"
        } else {
            "Origin"
        }),
    );
}

fn add_allowed_cors_headers(response: &mut Response) {
    response.headers_mut().insert(
        cors_header("access-control-allow-origin"),
        HeaderValue::from_static(MARKETING_ORIGIN),
    );
    response.headers_mut().insert(
        cors_header("access-control-allow-credentials"),
        HeaderValue::from_static("true"),
    );
}

fn rejected(status: StatusCode, message: &'static str, preflight: bool) -> Response {
    let mut response = (status, axum::Json(serde_json::json!({ "error": message }))).into_response();
    add_non_cacheable_headers(&mut response, preflight);
    response
}

fn dashboard_url(state: &WebState) -> String {
    state
        .browser_auth
        .as_ref()
        .map(|config| format!("{}/dashboard", config.public_origin.trim_end_matches('/')))
        .unwrap_or_else(|| DEFAULT_DASHBOARD_URL.to_owned())
}

#[derive(Debug, Serialize)]
struct SessionStatus {
    authenticated: bool,
    dashboard_url: String,
    check_after_seconds: u64,
}

fn status_document(authenticated: bool, dashboard_url: String) -> SessionStatus {
    SessionStatus {
        authenticated,
        dashboard_url,
        check_after_seconds: CHECK_AFTER_SECONDS,
    }
}

pub async fn get(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    if !origin_is_allowed(&headers) {
        return rejected(StatusCode::FORBIDDEN, "origin not allowed", false);
    }

    let viewer = session::resolve(&state, &headers).await;
    let mut response = axum::Json(status_document(viewer.is_signed_in(), dashboard_url(&state)))
        .into_response();
    add_non_cacheable_headers(&mut response, false);
    add_allowed_cors_headers(&mut response);
    response
}

pub async fn options(headers: HeaderMap) -> Response {
    if !origin_is_allowed(&headers) {
        return rejected(StatusCode::FORBIDDEN, "origin not allowed", true);
    }
    let method_is_get = headers
        .get(cors_header("access-control-request-method"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|method| method == "GET");
    if !method_is_get {
        return rejected(
            StatusCode::METHOD_NOT_ALLOWED,
            "only GET may be preflighted",
            true,
        );
    }
    if headers.contains_key(cors_header("access-control-request-headers")) {
        return rejected(
            StatusCode::FORBIDDEN,
            "custom request headers are not allowed",
            true,
        );
    }

    let mut response = StatusCode::NO_CONTENT.into_response();
    add_non_cacheable_headers(&mut response, true);
    add_allowed_cors_headers(&mut response);
    response.headers_mut().insert(
        cors_header("access-control-allow-methods"),
        HeaderValue::from_static("GET"),
    );
    response.headers_mut().insert(
        cors_header("access-control-max-age"),
        HeaderValue::from_static("300"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::Router;
    use serde_json::Value;
    use tower::util::ServiceExt;

    fn browser_auth() -> crate::state::BrowserAuthConfig {
        crate::state::BrowserAuthConfig {
            shared_auth_url: "http://shared-auth.internal".into(),
            shared_auth_public_url: "https://auth.example.test".into(),
            public_origin: "https://app.zpkg.net".into(),
            api_url: "http://zed-api.internal".into(),
            handoff_client_id: "zpkg".into(),
            handoff_client_secret: "test-only-secret".into(),
            delegate_client_id: "zpkg-web".into(),
            audience: "zed-pkg".into(),
            scopes: vec!["zpkg:account".into()],
            session_signing_secret: "0123456789abcdef0123456789abcdef".into(),
            session_cookie_name: "__Host-zpkg_session".into(),
            login_cookie_name: "__Host-zpkg_login".into(),
            secure_cookies: true,
        }
    }

    fn app() -> Router {
        crate::routes::router(Arc::new(WebState {
            db: None,
            registry_url: String::new(),
            shared_auth_url: None,
            session_path: "/auth/browser/session".into(),
            browser_auth: Some(browser_auth()),
            http: crate::proxy::client(),
        }))
    }

    async fn json(response: Response) -> Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    fn request(origin: Option<&str>) -> axum::http::Request<Body> {
        let mut builder = axum::http::Request::builder().uri("/auth/session/status");
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn exact_marketing_origin_gets_only_token_blind_status() {
        let response = app()
            .oneshot(request(Some(MARKETING_ORIGIN)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::VARY], "Origin");
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            MARKETING_ORIGIN
        );
        assert_eq!(response.headers()["access-control-allow-credentials"], "true");
        assert!(response.headers().get(header::SET_COOKIE).is_none());

        let document = json(response).await;
        let keys = document
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            ["authenticated", "check_after_seconds", "dashboard_url"]
                .into_iter()
                .collect()
        );
        assert_eq!(document["authenticated"], false);
        assert_eq!(document["dashboard_url"], "https://app.zpkg.net/dashboard");
        assert_eq!(document["check_after_seconds"], 3000);
        let serialized = document.to_string();
        for forbidden in ["token", "session_id", "user", "email", "role", "expires_at"] {
            assert!(!serialized.contains(forbidden), "{forbidden}");
        }
    }

    #[tokio::test]
    async fn missing_or_different_origins_get_no_auth_disclosure_or_cors_grant() {
        for origin in [None, Some("https://evil.example"), Some("null")] {
            let response = app().oneshot(request(origin)).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin:?}");
            assert!(response
                .headers()
                .get("access-control-allow-origin")
                .is_none());
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let document = json(response).await;
            assert_eq!(document, serde_json::json!({ "error": "origin not allowed" }));
        }
    }

    #[tokio::test]
    async fn preflight_is_exact_and_rejects_custom_headers() {
        let preflight = axum::http::Request::builder()
            .method("OPTIONS")
            .uri("/auth/session/status")
            .header(header::ORIGIN, MARKETING_ORIGIN)
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .unwrap();
        let response = app().oneshot(preflight).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["access-control-allow-methods"], "GET");
        assert_eq!(response.headers()["access-control-max-age"], "300");

        let with_custom_header = axum::http::Request::builder()
            .method("OPTIONS")
            .uri("/auth/session/status")
            .header(header::ORIGIN, MARKETING_ORIGIN)
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "authorization")
            .body(Body::empty())
            .unwrap();
        let response = app().oneshot(with_custom_header).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
    }

    #[test]
    fn status_document_has_only_coarse_non_identity_state() {
        let anonymous = status_document(false, DEFAULT_DASHBOARD_URL.to_owned());
        let authenticated = status_document(true, DEFAULT_DASHBOARD_URL.to_owned());
        assert!(!anonymous.authenticated);
        assert!(authenticated.authenticated);
        assert_eq!(anonymous.check_after_seconds, 3000);
        assert_eq!(authenticated.dashboard_url, DEFAULT_DASHBOARD_URL);
    }
}
