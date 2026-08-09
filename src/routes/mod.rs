//! Route table and the transport policy wrapped around it.
//!
//! One page per module. Handlers resolve the viewer, run named reads through
//! `zed-orm-core`, and render — they never mutate. Every mutation on this site
//! is a form that posts to the API server, because the web tier's database
//! identity is SELECT-only and `zed-orm-core` will not compile a write for it.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderValue, StatusCode, header};
use axum::routing::{any, get};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::proxy;
use crate::state::WebState;

mod console;
mod health;
mod home;
mod package;
mod search;

/// `unsafe-inline` is deliberately absent: HTMX is served from /static and all
/// styling is in styles.css, so nothing needs an inline allowance.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data: https:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; \
     object-src 'none'";

const STRICT_TRANSPORT_SECURITY: &str = "max-age=63072000; includeSubDomains";

fn security_header(
    name: header::HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

pub fn router(state: Arc<WebState>) -> Router {
    // Static security headers apply to the site only: the proxied auth pages
    // set their own (the upstream CSP must win, not be overridden here).
    let site = Router::new()
        .route("/", get(home::page))
        .route("/healthz", get(health::healthz))
        .route("/search", get(search::page))
        .route("/partials/search", get(search::partial))
        // Public package page. Kept at the short /p/ prefix it has always had.
        .route("/p/{org}/{name}", get(package::page))
        // Console.
        .route("/dashboard/{org}", get(console::dashboard))
        .route("/orgs/{org}", get(console::org_redirect))
        .route("/orgs/{org}/settings", get(console::org_settings))
        .route(
            "/orgs/{org}/projects/{project}/settings",
            get(console::project_settings),
        )
        .route(
            "/orgs/{org}/packages/{name}/settings",
            get(console::package_settings),
        )
        .route("/settings", get(console::user_settings))
        .nest_service("/static", tower_http::services::ServeDir::new("static"))
        .layer(security_header(
            header::CONTENT_SECURITY_POLICY,
            CONTENT_SECURITY_POLICY,
        ))
        .layer(security_header(
            header::STRICT_TRANSPORT_SECURITY,
            STRICT_TRANSPORT_SECURITY,
        ))
        .layer(security_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(security_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(security_header(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ));

    // Three literal forms, because a single `{*rest}` capture would not match
    // the bare `/shared-auth` and `/shared-auth/` paths.
    let shared_auth = Router::new()
        .route("/shared-auth", any(proxy::forward))
        .route("/shared-auth/", any(proxy::forward))
        .route("/shared-auth/{*rest}", any(proxy::forward));

    site.merge(shared_auth)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        // A panic in a handler becomes a 500 instead of a dropped connection.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
        .with_state(state)
}
