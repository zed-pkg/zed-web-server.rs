//! Route table and the transport policy wrapped around it.
//!
//! One page per module. Page handlers resolve the viewer and run named reads
//! through `zed-orm-core`; the database boundary stays SELECT-only. Mutations
//! terminate at same-origin BFF routes, which rotate/delegate Shared Auth
//! credentials and forward canonical JSON requests to the write-enabled API.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderValue, StatusCode, header};
use axum::routing::{any, get, post};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::browser_auth;
use crate::proxy;
use crate::state::WebState;

mod console;
mod health;
mod home;
mod package;
mod search;

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data: https:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; \
     object-src 'none'";
const STRICT_TRANSPORT_SECURITY: &str = "max-age=63072000; includeSubDomains";

fn security_header(
    name: header::HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

pub fn router(state: Arc<WebState>) -> Router {
    let site = Router::new()
        .route("/", get(home::page))
        .route("/healthz", get(health::healthz))
        .route("/search", get(search::page))
        .route("/partials/search", get(search::partial))
        .route("/p/{org}/{name}", get(package::page))
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
        // Exact-client Shared Auth handoff. The callback is backend-only; the
        // browser never receives the delegated zed-pkg API token.
        .route("/auth/sign-in", get(browser_auth::sign_in))
        .route("/auth/shared/callback", get(browser_auth::callback))
        .route("/auth/logout", post(browser_auth::logout))
        // Compatibility aliases used by the already-reviewed header markup.
        .route(
            "/shared-auth/auth/browser/sign-in",
            get(browser_auth::sign_in),
        )
        .route("/shared-auth/auth/logout", post(browser_auth::logout))
        // Stable same-origin form endpoints. Each handler translates the old
        // UI path to `/api/v1/account/*` after origin checks and delegation.
        .route("/v1/orgs", post(browser_auth::create_org))
        .route("/v1/orgs/{org}/invitations", post(browser_auth::invite_org))
        .route(
            "/v1/orgs/{org}/projects",
            post(browser_auth::create_project),
        )
        .route(
            "/v1/orgs/{org}/packages",
            post(browser_auth::create_package),
        )
        .route(
            "/v1/projects/{project_id}/invitations",
            post(browser_auth::invite_project),
        )
        .route(
            "/v1/orgs/{org}/packages/{package}",
            post(browser_auth::update_package),
        )
        .route(
            "/v1/orgs/{org}/packages/{package}/visibility",
            post(browser_auth::make_public),
        )
        .route("/v1/users/me", post(browser_auth::update_user))
        // Raw Shared Auth gateway for browser-owned pages/assets not handled by
        // the exact product routes above.
        .route("/shared-auth", any(proxy::forward))
        .route("/shared-auth/", any(proxy::forward))
        .route("/shared-auth/{*rest}", any(proxy::forward))
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

    site.layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
        .with_state(state)
}
