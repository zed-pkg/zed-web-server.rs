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
mod dependency_graph;
mod dependency_graph_page;
mod health;
mod home;
mod package;
mod search;
mod session_status;

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     connect-src 'self'; img-src 'self' data: https:; frame-ancestors 'none'; base-uri 'none'; \
     form-action 'self'; object-src 'none'";
const STRICT_TRANSPORT_SECURITY: &str = "max-age=63072000; includeSubDomains";
const SHARED_AUTH_UI_PREFIX: &str = "/shared-auth-ui";
const MARKETING_AUTH_ENTRY: &str = "/auth/sign-in?return_to=%2Fdashboard";
// These routes are stable rather than content-hashed. Revalidation prevents a
// deployment from being hidden behind a year-long cached copy of an older UI.
const GRAPH_ASSET_CACHE: &str = "public, max-age=300, must-revalidate";
const DEPENDENCY_GRAPH_JS: &[u8] = include_bytes!("../../assets/dependency-graph.js");
const DEPENDENCY_GRAPH_JS_BR: &[u8] = include_bytes!("../../static/dependency-graph.js.br");
const DEPENDENCY_GRAPH_CSS: &[u8] = include_bytes!("../../assets/dependency-graph.css");
const DEPENDENCY_GRAPH_CSS_BR: &[u8] = include_bytes!("../../static/dependency-graph.css.br");

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

/// Serve inspectable source assets without a JavaScript package manager or an
/// external CDN. The checked-in Brotli files are reproducible build outputs;
/// clients that do not advertise Brotli receive the authoritative source.
fn graph_asset(
    request_headers: &HeaderMap,
    content_type: &'static str,
    source: &'static [u8],
    brotli: &'static [u8],
) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(GRAPH_ASSET_CACHE),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    let bytes = if accepts_brotli(request_headers) {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
        brotli
    } else {
        source
    };
    (headers, bytes).into_response()
}

fn accepts_brotli(headers: &HeaderMap) -> bool {
    let mut explicit = None::<f32>;
    let mut wildcard = None::<f32>;
    for value in headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
    {
        for coding in value.split(',') {
            let mut parts = coding.split(';');
            let name = parts.next().unwrap_or_default().trim();
            let mut quality = 1.0;
            for parameter in parts {
                let Some((key, value)) = parameter.trim().split_once('=') else {
                    quality = 0.0;
                    continue;
                };
                if key.trim().eq_ignore_ascii_case("q") {
                    quality = value
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|value| (0.0..=1.0).contains(value))
                        .unwrap_or(0.0);
                }
            }
            let destination = if name.eq_ignore_ascii_case("br") {
                Some(&mut explicit)
            } else if name == "*" {
                Some(&mut wildcard)
            } else {
                None
            };
            if let Some(destination) = destination {
                *destination = Some(destination.unwrap_or(0.0).max(quality));
            }
        }
    }
    explicit.or(wildcard).is_some_and(|quality| quality > 0.0)
}

async fn dependency_graph_js(headers: HeaderMap) -> Response {
    graph_asset(
        &headers,
        "text/javascript; charset=utf-8",
        DEPENDENCY_GRAPH_JS,
        DEPENDENCY_GRAPH_JS_BR,
    )
}

async fn dependency_graph_css(headers: HeaderMap) -> Response {
    graph_asset(
        &headers,
        "text/css; charset=utf-8",
        DEPENDENCY_GRAPH_CSS,
        DEPENDENCY_GRAPH_CSS_BR,
    )
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
        .route(
            "/graph-assets/dependency-graph.js",
            get(dependency_graph_js),
        )
        .route(
            "/graph-assets/dependency-graph.css",
            get(dependency_graph_css),
        )
        .route("/search", get(search::page))
        .route("/partials/search", get(search::partial))
        .route("/p/{org}/{name}", get(package::page))
        .route("/dashboard/{org}", get(console::dashboard))
        .route(
            "/dashboard/{org}/dependency-graph",
            get(dependency_graph_page::organization),
        )
        .route("/orgs/{org}", get(console::org_redirect))
        .route("/orgs/{org}/settings", get(console::org_settings))
        .route(
            "/orgs/{org}/projects/{project}/settings",
            get(console::project_settings),
        )
        .route(
            "/orgs/{org}/projects/{project}/dependency-graph",
            get(dependency_graph_page::project),
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
        // Read-only dependency graph BFF. Every package route repeats the
        // page's visibility check before contacting the canonical API.
        .route(
            "/bff/dependency-graphs/packages/{org}/{name}/latest",
            get(dependency_graph::latest_package_document),
        )
        .route(
            "/bff/dependency-graphs/packages/{org}/{name}/{version}",
            get(dependency_graph::package_document),
        )
        .route(
            "/bff/dependency-graphs/packages/{org}/{name}/{version}/export/{format}",
            get(dependency_graph::package_export),
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

    #[test]
    fn graph_workspace_csp_remains_self_hosted() {
        assert!(CONTENT_SECURITY_POLICY.contains("script-src 'self'"));
        assert!(CONTENT_SECURITY_POLICY.contains("connect-src 'self'"));
        assert!(!CONTENT_SECURITY_POLICY.contains("unsafe-inline"));
        assert!(!CONTENT_SECURITY_POLICY.contains("claritas"));
    }

    #[test]
    fn graph_assets_negotiate_brotli_with_a_source_fallback() {
        let response = graph_asset(
            &HeaderMap::new(),
            "text/javascript; charset=utf-8",
            DEPENDENCY_GRAPH_JS,
            DEPENDENCY_GRAPH_JS_BR,
        );
        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            GRAPH_ASSET_CACHE
        );
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding"
        );

        let mut accepts = HeaderMap::new();
        accepts.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, br;q=0.8"),
        );
        let response = graph_asset(
            &accepts,
            "text/javascript; charset=utf-8",
            DEPENDENCY_GRAPH_JS,
            DEPENDENCY_GRAPH_JS_BR,
        );
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "br");
    }

    #[test]
    fn explicit_brotli_rejection_overrides_the_wildcard() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("*;q=1, br;q=0"),
        );
        assert!(!accepts_brotli(&headers));
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("BR;Q=0.5"),
        );
        assert!(accepts_brotli(&headers));
    }
}
