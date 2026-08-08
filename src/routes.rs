use std::sync::Arc;

use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get};
use maud::html;
use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Select};
use serde::Deserialize;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::entities::{org, package, version};
use crate::proxy;
use crate::state::WebState;
use crate::views::{
    PackageRow, VersionRow, install_snippet, layout, package_rows, search_box, version_table,
};

/// Upper bound on rows fetched for an org's package listing and a package's
/// version listing. Mirrors the caps on the search (50) and recent (20) paths
/// so a single org/package can't force an unbounded scan.
const PAGE_LIMIT: u64 = 100;

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; \
     object-src 'none'";

const STRICT_TRANSPORT_SECURITY: &str = "max-age=63072000; includeSubDomains";

/// Layer that sets a static security header on every response.
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
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/search", get(search_page))
        .route("/partials/search", get(search_partial))
        .route("/p/{org}/{name}", get(package_page))
        .route("/orgs/{org}", get(org_page))
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

    let shared_auth = Router::new()
        .route("/shared-auth", any(proxy::forward))
        .route("/shared-auth/{*rest}", any(proxy::forward));

    site.merge(shared_auth)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        // Turn a panic in a handler into a graceful 500 instead of dropping the
        // connection.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        // Cap the wall-clock time any single request may occupy a worker.
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
        .with_state(state)
}

async fn healthz(State(state): State<Arc<WebState>>) -> axum::Json<serde_json::Value> {
    let db = match &state.db {
        Some(db) => db.ping().await.is_ok(),
        None => false,
    };
    axum::Json(serde_json::json!({ "ok": true, "db": db, "registry": state.registry_url }))
}

async fn recent_packages(state: &WebState, limit: u64) -> Vec<PackageRow> {
    let Some(db) = &state.db else {
        return Vec::new();
    };
    let Ok(rows) = package::Entity::find()
        .find_also_related(org::Entity)
        .order_by_desc(package::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await
    else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(rows.len());
    for (pkg, org_row) in rows {
        let Some(org_row) = org_row else { continue };
        let latest = version::Entity::find()
            .filter(version::Column::PackageId.eq(pkg.id))
            .filter(version::Column::Yanked.eq(false))
            .order_by_desc(version::Column::PublishedAt)
            .one(db)
            .await
            .ok()
            .flatten()
            .map(|v| v.version);
        out.push(PackageRow {
            org: org_row.slug,
            name: pkg.name,
            description: pkg.description,
            latest,
        });
    }
    out
}

async fn home(State(state): State<Arc<WebState>>) -> Html<String> {
    let recent = recent_packages(&state, 20).await;
    let content = html! {
        section class="hero" {
            h1 { "Install " span class="orange" { "packages" } ", not repositories." }
            p class="muted" {
                "The zed-pkg registry: lean, tag-verified artifacts backed by the VCS "
                "hosts you already use. One store per machine, symlinks per project."
            }
            (install_snippet("acme", "http-kit"))
        }
        section {
            h2 { "Recently published" }
            (package_rows(&recent, "nothing published yet; `zed publish` something"))
        }
    };
    Html(layout("zed-pkg registry", state.db.is_some(), content).into_string())
}

#[derive(Deserialize)]
struct SearchParams {
    #[serde(default)]
    q: String,
}

/// Escape `\`, `%`, and `_` so user input matches literally inside a SQL
/// LIKE pattern (SeaORM's `contains` wraps the value in `%...%` unescaped).
fn escape_like(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len());
    for ch in query.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

async fn find_packages(state: &WebState, query: &str) -> Vec<PackageRow> {
    let Some(db) = &state.db else {
        return Vec::new();
    };
    let pattern = escape_like(query);
    let Ok(rows) = package::Entity::find()
        .filter(
            Condition::any()
                .add(package::Column::Name.contains(&pattern))
                .add(package::Column::Description.contains(&pattern)),
        )
        .find_also_related(org::Entity)
        .limit(50)
        .all(db)
        .await
    else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|(pkg, org_row)| {
            org_row.map(|org_row| PackageRow {
                org: org_row.slug,
                name: pkg.name,
                description: pkg.description,
                latest: None,
            })
        })
        .collect()
}

async fn search_page(
    State(state): State<Arc<WebState>>,
    Query(params): Query<SearchParams>,
) -> Html<String> {
    let results = find_packages(&state, &params.q).await;
    let content = html! {
        section {
            h1 { "Search" }
            (search_box(&params.q))
            div id="initial-results" { (package_rows(&results, "type to search")) }
        }
    };
    Html(layout("search - zed-pkg", state.db.is_some(), content).into_string())
}

async fn search_partial(
    State(state): State<Arc<WebState>>,
    Query(params): Query<SearchParams>,
) -> Html<String> {
    let results = find_packages(&state, &params.q).await;
    Html(package_rows(&results, "no matches").into_string())
}

/// Only http(s) URLs are worth turning into hyperlinks; anything else
/// (`javascript:`, `data:`, ssh remotes, ...) is rendered as plain text.
fn is_linkable_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Bounded query for the packages that belong to an org, newest first. Capped
/// at `PAGE_LIMIT` so a single org can't force an unbounded table scan.
fn org_packages_query(org_id: sea_orm::prelude::Uuid) -> Select<package::Entity> {
    package::Entity::find()
        .filter(package::Column::OrgId.eq(org_id))
        .order_by_desc(package::Column::CreatedAt)
        .limit(PAGE_LIMIT)
}

/// Bounded query for a package's versions, newest first. Capped at `PAGE_LIMIT`;
/// the semver sort then runs over just this page rather than the whole table.
fn package_versions_query(package_id: sea_orm::prelude::Uuid) -> Select<version::Entity> {
    version::Entity::find()
        .filter(version::Column::PackageId.eq(package_id))
        .order_by_desc(version::Column::PublishedAt)
        .limit(PAGE_LIMIT)
}

async fn package_page(
    State(state): State<Arc<WebState>>,
    Path((org_slug, name)): Path<(String, String)>,
) -> Response {
    let mut description = None;
    let mut repo = None;
    let mut versions: Vec<VersionRow> = Vec::new();
    // Offline mode renders the page as before (200 with an empty body); with a
    // DB, a genuinely-missing org/package yields a 404 and a DbErr a 500.
    let mut found = state.db.is_none();

    if let Some(db) = &state.db {
        let org_row = match org::Entity::find()
            .filter(org::Column::Slug.eq(&org_slug))
            .one(db)
            .await
        {
            Ok(Some(org_row)) => Some(org_row),
            Ok(None) => None,
            Err(error) => {
                tracing::error!(%error, org = %org_slug, "package_page: org lookup failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        if let Some(org_row) = org_row {
            let pkg = match package::Entity::find()
                .filter(package::Column::OrgId.eq(org_row.id))
                .filter(package::Column::Name.eq(&name))
                .one(db)
                .await
            {
                Ok(Some(pkg)) => Some(pkg),
                Ok(None) => None,
                Err(error) => {
                    tracing::error!(%error, org = %org_slug, package = %name, "package_page: package lookup failed");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };

            if let Some(pkg) = pkg {
                found = true;
                description = pkg.description.clone();
                repo = Some((pkg.vcs.clone(), pkg.repo_url.clone()));
                match package_versions_query(pkg.id).all(db).await {
                    Ok(rows) => {
                        versions = rows
                            .into_iter()
                            .map(|v| VersionRow {
                                version: v.version,
                                published_at: v.published_at.format("%Y-%m-%d").to_string(),
                                size: v.size,
                                sha256: v.sha256,
                                vcs_tag: v.vcs_tag,
                                yanked: v.yanked,
                            })
                            .collect();
                        versions.sort_by(|a, b| {
                            let pa = semver::Version::parse(&a.version).ok();
                            let pb = semver::Version::parse(&b.version).ok();
                            pb.cmp(&pa)
                        });
                    }
                    Err(error) => {
                        tracing::error!(%error, org = %org_slug, package = %name, "package_page: version lookup failed");
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                }
            }
        }
    }

    let content = html! {
        section {
            h1 class="mono" { (org_slug) "/" (name) }
            @if let Some(description) = &description { p class="muted" { (description) } }
            @if let Some((vcs, url)) = &repo {
                p class="muted" {
                    "backed by " span class="blue mono" { (vcs) } " at "
                    @if is_linkable_url(url) { a href=(url) { (url) } } @else { (url) }
                }
            }
            (install_snippet(&org_slug, &name))
            @if versions.is_empty() {
                p class="muted" { "no published versions found" }
            } @else {
                (version_table(&versions))
            }
        }
    };
    let body = Html(
        layout(
            &format!("{org_slug}/{name} - zed-pkg"),
            state.db.is_some(),
            content,
        )
        .into_string(),
    );
    if found {
        body.into_response()
    } else {
        (StatusCode::NOT_FOUND, body).into_response()
    }
}

async fn org_page(State(state): State<Arc<WebState>>, Path(org_slug): Path<String>) -> Response {
    let mut packages: Vec<PackageRow> = Vec::new();
    // Offline mode keeps the old 200-with-empty-body behaviour; with a DB a
    // missing org is a 404 and a DbErr a 500.
    let mut found = state.db.is_none();

    if let Some(db) = &state.db {
        let org_row = match org::Entity::find()
            .filter(org::Column::Slug.eq(&org_slug))
            .one(db)
            .await
        {
            Ok(Some(org_row)) => Some(org_row),
            Ok(None) => None,
            Err(error) => {
                tracing::error!(%error, org = %org_slug, "org_page: org lookup failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        if let Some(org_row) = org_row {
            match org_packages_query(org_row.id).all(db).await {
                Ok(rows) => {
                    found = true;
                    packages = rows
                        .into_iter()
                        .map(|pkg| PackageRow {
                            org: org_slug.clone(),
                            name: pkg.name,
                            description: pkg.description,
                            latest: None,
                        })
                        .collect();
                }
                Err(error) => {
                    tracing::error!(%error, org = %org_slug, "org_page: package lookup failed");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    }

    let content = html! {
        section {
            h1 class="mono" { (org_slug) }
            (package_rows(&packages, "org not found or empty"))
        }
    };
    let body = Html(
        layout(
            &format!("{org_slug} - zed-pkg"),
            state.db.is_some(),
            content,
        )
        .into_string(),
    );
    if found {
        body.into_response()
    } else {
        (StatusCode::NOT_FOUND, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::util::ServiceExt;

    fn offline_state() -> Arc<WebState> {
        Arc::new(WebState {
            db: None,
            registry_url: "https://registry.zpkg.net".into(),
            shared_auth_url: None,
            http: crate::proxy::client(),
        })
    }

    async fn body_of(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn home_renders_offline_mode() {
        let app = router(offline_state());
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_of(response).await;
        assert!(body.contains("registry offline"));
        assert!(body.contains("not repositories"));
    }

    #[tokio::test]
    async fn responses_carry_security_headers() {
        let app = router(offline_state());
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let headers = response.headers();
        assert_eq!(
            headers["content-security-policy"],
            "default-src 'self'; script-src 'self'; style-src 'self'; \
             img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; \
             object-src 'none'"
        );
        assert_eq!(
            headers["strict-transport-security"],
            "max-age=63072000; includeSubDomains"
        );
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(
            headers["referrer-policy"],
            "strict-origin-when-cross-origin"
        );
    }

    #[test]
    fn org_and_version_queries_are_bounded() {
        use sea_orm::{DatabaseBackend, QueryTrait};

        let id = sea_orm::prelude::Uuid::nil();
        let org_sql = org_packages_query(id).build(DatabaseBackend::Postgres).sql;
        assert!(
            org_sql.contains("LIMIT"),
            "org query missing LIMIT: {org_sql}"
        );
        let version_sql = package_versions_query(id)
            .build(DatabaseBackend::Postgres)
            .sql;
        assert!(
            version_sql.contains("LIMIT"),
            "version query missing LIMIT: {version_sql}"
        );
    }

    #[tokio::test]
    async fn unknown_org_returns_404() {
        use sea_orm::{DatabaseBackend, MockDatabase};

        // Org lookup returns zero rows -> genuinely-missing org -> 404
        // (as opposed to offline mode, which renders 200).
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<org::Model>::new()])
            .into_connection();
        let state = Arc::new(WebState {
            db: Some(db),
            registry_url: "https://registry.zpkg.net".into(),
        });
        let app = router(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/orgs/does-not-exist")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn escape_like_makes_wildcards_literal() {
        assert_eq!(escape_like("http"), "http");
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("c:\\dir"), "c:\\\\dir");
        assert_eq!(escape_like("%_\\"), "\\%\\_\\\\");
    }

    #[test]
    fn only_http_urls_are_linkable() {
        assert!(is_linkable_url("https://github.com/acme/http-kit"));
        assert!(is_linkable_url("http://internal.example"));
        assert!(!is_linkable_url("javascript:alert(1)"));
        assert!(!is_linkable_url("git@github.com:acme/http-kit.git"));
        assert!(!is_linkable_url("ssh://git@github.com/acme/http-kit"));
    }

    #[tokio::test]
    async fn healthz_reports_db_false_when_offline() {
        let app = router(offline_state());
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/healthz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_of(response).await;
        assert!(body.contains("\"db\":false"));
    }
}
