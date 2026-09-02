//! Route table and the transport policy wrapped around it.
//!
//! One page per module. Page handlers resolve the viewer and run named reads
//! through `zed-orm-core`; the database boundary stays SELECT-only. Mutations
//! terminate at same-origin BFF routes, which rotate/delegate Shared Auth
//! credentials and forward canonical JSON requests to the write-enabled API.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes, HttpBody};
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use http_body::Frame;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::browser_auth;
use crate::proxy;
use crate::state::WebState;

mod console;
mod dependency_graph;
mod dependency_graph_fragments;
mod dependency_graph_page;
mod health;
mod home;
mod package;
mod search;
mod session_status;
mod storage;

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
/// The service worker is served from the origin root, not from `/static`.
///
/// A worker's default scope is the directory it was served from, so a copy at
/// `/static/sw.js` could only ever control `/static/*` — useless for the
/// offline navigation fallback. Serving the same bytes at `/sw.js` gives it
/// root scope without needing a `Service-Worker-Allowed` header.
const SERVICE_WORKER_JS: &[u8] = include_bytes!("../../static/sw.js");
const DEPENDENCY_GRAPH_JS_BR: &[u8] = include_bytes!("../../static/dependency-graph.js.br");
const DEPENDENCY_GRAPH_CSS: &[u8] = include_bytes!("../../assets/dependency-graph.css");
const DEPENDENCY_GRAPH_CSS_BR: &[u8] = include_bytes!("../../static/dependency-graph.css.br");

/// An empty response body whose size is deliberately unknown to Hyper.
///
/// Hyper strips an explicit `Content-Length` from a known-empty 304 response,
/// even though that field describes the selected representation rather than a
/// message body for this status. Keeping the size hint unknown preserves that
/// metadata on the wire while `poll_frame` guarantees no bytes are emitted.
#[derive(Debug)]
struct MetadataOnlyBody;

impl HttpBody for MetadataOnlyBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(None)
    }
}

fn not_modified_with_representation_metadata(headers: HeaderMap) -> Response {
    let mut response = Response::new(Body::new(MetadataOnlyBody));
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    *response.headers_mut() = headers;
    response
}

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
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&bytes.len().to_string())
            .expect("a static asset length is a valid Content-Length"),
    );
    let etag = format!("\"{:016x}\"", fnv1a(bytes));
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    if if_none_match_matches(request_headers, &etag) {
        return not_modified_with_representation_metadata(headers);
    }
    (headers, bytes).into_response()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn if_none_match_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
        })
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

/// `GET /sw.js` — the offline shell worker, at root scope.
///
/// Revalidated rather than cached hard: a worker pinned for a year would keep
/// serving a superseded precache list long after a deploy, and the worker is
/// the one asset that cannot be corrected by shipping a new page.
async fn service_worker(request_headers: HeaderMap) -> Response {
    // Deliberately not routed through `graph_asset`: that helper labels the
    // response `Content-Encoding: br` whenever the client accepts Brotli, and
    // no Brotli variant of this file is built. Serving identity bytes under a
    // Brotli label would break the worker for every modern browser.
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(GRAPH_ASSET_CACHE),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&SERVICE_WORKER_JS.len().to_string())
            .expect("a static asset length is a valid Content-Length"),
    );
    let etag = format!("\"{:016x}\"", fnv1a(SERVICE_WORKER_JS));
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    if if_none_match_matches(&request_headers, &etag) {
        // Reuses the shared 304 body so representation metadata survives the
        // conditional response without smuggling a payload alongside it.
        return not_modified_with_representation_metadata(headers);
    }
    (headers, SERVICE_WORKER_JS).into_response()
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
        .route("/sw.js", get(service_worker))
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
        // Presentation-only graph fragments. The browser posts a bounded view
        // of its already-authorized model; a 256 KiB ceiling keeps this HTML
        // renderer out of the graph data plane.
        .route(
            "/partials/dependency-graph/query",
            post(dependency_graph_fragments::query).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/partials/dependency-graph/inspector",
            post(dependency_graph_fragments::inspector).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/partials/dependency-graph/table",
            post(dependency_graph_fragments::table).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/partials/dependency-graph/state",
            post(dependency_graph_fragments::state).layer(DefaultBodyLimit::max(16 * 1024)),
        )
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
        // Provider-agnostic artifact storage console. Reads the registry API's
        // own report rather than embedding any vendor dashboard.
        .route("/console/storage", get(storage::console))
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
        // The installable shell adds a manifest and a service worker. Both fall
        // back to `default-src 'self'` (manifest-src directly, worker-src via
        // child-src then script-src), so the policy needs no widening — this
        // asserts the fallback that makes that true has not been narrowed.
        assert!(CONTENT_SECURITY_POLICY.contains("default-src 'self'"));
        assert!(!CONTENT_SECURITY_POLICY.contains("manifest-src"));
        assert!(!CONTENT_SECURITY_POLICY.contains("worker-src"));
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

    #[tokio::test]
    async fn graph_assets_revalidate_each_content_encoding_with_a_strong_etag() {
        let first = graph_asset(
            &HeaderMap::new(),
            "text/javascript; charset=utf-8",
            DEPENDENCY_GRAPH_JS,
            DEPENDENCY_GRAPH_JS_BR,
        );
        let source_etag = first.headers()[header::ETAG].clone();
        assert!(!source_etag.as_bytes().starts_with(b"W/"));

        let mut conditional = HeaderMap::new();
        conditional.insert(header::IF_NONE_MATCH, source_etag.clone());
        let not_modified = graph_asset(
            &conditional,
            "text/javascript; charset=utf-8",
            DEPENDENCY_GRAPH_JS,
            DEPENDENCY_GRAPH_JS_BR,
        );
        assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(not_modified.headers()[header::ETAG], source_etag);
        assert_eq!(
            not_modified.headers()[header::CONTENT_LENGTH],
            DEPENDENCY_GRAPH_JS.len().to_string()
        );
        assert!(not_modified.body().size_hint().upper().is_none());
        let body = axum::body::to_bytes(not_modified.into_body(), 1)
            .await
            .expect("metadata-only body is readable");
        assert!(body.is_empty());

        conditional.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("br"));
        let compressed = graph_asset(
            &conditional,
            "text/javascript; charset=utf-8",
            DEPENDENCY_GRAPH_JS,
            DEPENDENCY_GRAPH_JS_BR,
        );
        assert_eq!(compressed.status(), StatusCode::OK);
        assert_ne!(compressed.headers()[header::ETAG], source_etag);
        assert_eq!(compressed.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(
            compressed.headers()[header::CONTENT_LENGTH],
            DEPENDENCY_GRAPH_JS_BR.len().to_string()
        );
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
