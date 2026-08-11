//! Route table and the transport policy wrapped around it.
//!
//! One page per module. Page handlers resolve the viewer and run named reads
//! through `zed-orm-core`; the database boundary stays SELECT-only. Mutations
//! terminate at same-origin BFF routes, which rotate/delegate Shared Auth
//! credentials and forward canonical JSON requests to the write-enabled API.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
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
mod session_status;

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data: https:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; \
     object-src 'none'";
const STRICT_TRANSPORT_SECURITY: &str = "max-age=63072000; includeSubDomains";
const SHARED_AUTH_UI_PREFIX: &str = "/shared-auth-ui";
const MARKETING_AUTH_ENTRY: &str = "/auth/sign-in?return_to=%2Fdashboard";

fn security_header(
    name: header::HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

/// Stable marketing entry used by both "Log in" and "Sign up". Shared Auth
/// decides whether a verified email resumes an account or creates one according
/// to the registered customer-realm policy; the static site never handles a
/// provider token or secret.
async fn marketing_auth_entry() -> Redirect {
    Redirect::temporary(MARKETING_AUTH_ENTRY)
}

/// `/dashboard` is intentionally organization-neutral. Resolve the product
/// viewer server-side and send an existing member to the first visible org. A
/// signed-in user without an org gets the personal settings/onboarding surface;
/// an anonymous browser begins the PKCE/BFF ceremony.
async fn marketing_dashboard(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let viewer = crate::session::resolve(&state, &headers).await;
    if !viewer.is_signed_in() {
        return Redirect::temporary(MARKETING_AUTH_ENTRY).into_response();
    }
    if let Some(org) = viewer.orgs().first() {
        return Redirect::temporary(&format!("/dashboard/{}", org.slug)).into_response();
    }
    Redirect::temporary("/settings").into_response()
}

/// Convert the dedicated same-origin Shared Auth UI prefix into the legacy
/// internal proxy prefix. `proxy::forward` then removes `/shared-auth` before
/// contacting the service. Shared Auth must render links with
/// `AUTH_BROWSER_PUBLIC_PREFIX=/shared-auth-ui`, so redirects return through
/// this route rather than through the PKCE/BFF callback.
fn shared_auth_ui_upstream_uri(uri: &Uri) -> Result<Uri, axum::http::uri::InvalidUri> {
    let service_path = shared_auth_ui_service_path(uri).unwrap_or("/");
    let query = uri
        .query()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    format!("/shared-auth{service_path}{query}").parse()
}

fn shared_auth_ui_service_path(uri: &Uri) -> Option<&str> {
    let rest = uri.path().strip_prefix(SHARED_AUTH_UI_PREFIX)?;
    Some(if rest.is_empty() { "/" } else { rest })
}

/// Only browser-ceremony routes are exposed through the dedicated prefix.
/// Redemption, exchange, delegation, refresh, introspection, metrics, and
/// internal webhooks remain cluster-internal back-channel endpoints.
fn shared_auth_ui_path_allowed(path: &str) -> bool {
    matches!(
        path,
        "/" | "/ui" | "/auth/browser/sign-in" | "/auth/browser/consume" | "/auth/browser/otp"
    )
}

async fn forward_shared_auth_ui(
    State(state): State<Arc<WebState>>,
    mut request: Request,
) -> Response {
    let Some(service_path) = shared_auth_ui_service_path(request.uri()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !shared_auth_ui_path_allowed(service_path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(uri) = shared_auth_ui_upstream_uri(request.uri()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    *request.uri_mut() = uri;
    proxy::forward(State(state), request).await
}

pub fn router(state: Arc<WebState>) -> Router {
    let site = Router::new()
        .route("/", get(home::page))
        // Stable account destinations used by zpkg.net. They terminate at the
        // Rust web server, never in static JavaScript.
        .route("/login", get(marketing_auth_entry))
        .route("/signup", get(marketing_auth_entry))
        .route("/dashboard", get(marketing_dashboard))
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
        // Exact, credentialed CORS surface for the static marketing header.
        // It returns only a boolean, dashboard URL, and coarse recheck hint.
        .route(
            "/auth/session/status",
            get(session_status::get).options(session_status::options),
        )
        // Exact-client Shared Auth handoff. The callback is backend-only; the
        // browser never receives the delegated zed-pkg API token.
        .route("/auth/sign-in", get(browser_auth::sign_in))
        .route("/auth/shared/callback", get(browser_auth::callback))
        .route("/auth/logout", post(browser_auth::logout))
        // Compatibility aliases used by the already-reviewed header markup.
        // These aliases remain PKCE/BFF routes. The distinct `/shared-auth-ui`
        // namespace below is the actual same-origin proxied ceremony.
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
        // Dedicated same-origin Shared Auth browser ceremony. It has its own
        // public prefix, never invokes `/auth/shared/callback`, and exposes only
        // the browser routes above; confidential APIs stay on the cluster URL.
        .route("/shared-auth-ui", any(forward_shared_auth_ui))
        .route("/shared-auth-ui/", any(forward_shared_auth_ui))
        .route("/shared-auth-ui/{*rest}", any(forward_shared_auth_ui))
        // Raw Shared Auth gateway retained for compatibility with already
        // reviewed pages/assets. New integrations should use `/shared-auth-ui`
        // for the browser ceremony and the cluster-internal URL for back-channel
        // APIs instead of depending on this broad legacy surface.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marketing_account_entry_is_local_and_returns_to_dashboard() {
        assert_eq!(MARKETING_AUTH_ENTRY, "/auth/sign-in?return_to=%2Fdashboard");
        assert!(MARKETING_AUTH_ENTRY.starts_with('/'));
        assert!(!MARKETING_AUTH_ENTRY.starts_with("//"));
    }

    #[test]
    fn shared_auth_ui_prefix_preserves_path_and_query() {
        let root: Uri = "/shared-auth-ui".parse().unwrap();
        assert_eq!(shared_auth_ui_upstream_uri(&root).unwrap(), "/shared-auth/");

        let sign_in: Uri = "/shared-auth-ui/auth/browser/sign-in?return=%2Fdashboard"
            .parse()
            .unwrap();
        assert_eq!(
            shared_auth_ui_upstream_uri(&sign_in).unwrap(),
            "/shared-auth/auth/browser/sign-in?return=%2Fdashboard"
        );
    }

    #[test]
    fn shared_auth_ui_prefix_exposes_only_browser_ceremony_routes() {
        for allowed in [
            "/",
            "/ui",
            "/auth/browser/sign-in",
            "/auth/browser/consume",
            "/auth/browser/otp",
        ] {
            assert!(shared_auth_ui_path_allowed(allowed), "{allowed}");
        }
        for internal in [
            "/auth/handoff/redeem",
            "/auth/exchange",
            "/auth/delegate",
            "/auth/refresh",
            "/auth/introspect",
            "/internal/webhook/sync",
            "/metrics",
        ] {
            assert!(!shared_auth_ui_path_allowed(internal), "{internal}");
        }
    }
}
