//! `/p/{org}/{name}` — the public package page.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use maud::html;
use zed_orm_core::models::PackageSummary;

use crate::session::{self, Viewer};
use crate::state::WebState;
use crate::views::{PageContext, components, dependency_graph, layout};

/// Adapt a data-plane summary to the list row the templates render.
pub fn summary_row(summary: &PackageSummary) -> components::PackageRow {
    components::PackageRow {
        org: summary.org_slug.clone(),
        name: summary.name.clone(),
        description: summary.description.clone(),
        latest: summary.latest_version.clone(),
        visibility: summary.visibility.clone(),
    }
}

/// Only http(s) links are turned into anchors; anything else renders as text so
/// a stored `javascript:` or `data:` URL cannot become a live link.
fn is_linkable_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn is_prerelease(version: &str, scheme: &str) -> bool {
    let scheme = zed_interfaces::version::VersionScheme::from_str_lenient(scheme);
    if scheme == zed_interfaces::version::VersionScheme::Opaque {
        return false;
    }
    zed_interfaces::version::parse_version(version).is_some_and(|parsed| !parsed.pre.is_empty())
}

pub async fn page(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org_slug, name)): Path<(String, String)>,
) -> Response {
    let viewer = session::resolve(&state, &headers).await;

    let Some(db) = &state.db else {
        return offline(&state, &viewer, &org_slug, &name);
    };

    let found = zed_orm_core::read::package_by_org_and_name(db, &org_slug, &name)
        .await
        .unwrap_or(None);

    let Some((package, org)) = found else {
        return not_found(&state, &viewer, &org_slug, &name);
    };

    // A private package is visible only to members of its org. Anonymous and
    // non-member viewers get the same 404 as a package that does not exist, so
    // the page cannot be used to probe for private names.
    if package.visibility != "public" && !viewer.can_see_private(&org.slug) {
        return not_found(&state, &viewer, &org_slug, &name);
    }

    let versions = zed_orm_core::read::versions_for_package(db, package.id)
        .await
        .unwrap_or_default();
    let licenses = zed_orm_core::read::licenses_for_package(db, package.id)
        .await
        .unwrap_or_default();

    let rows: Vec<components::VersionRow> = versions
        .iter()
        .map(|version| components::VersionRow {
            version: version.version.clone(),
            published_at: version.published_at.format("%Y-%m-%d").to_string(),
            size: version.size_bytes,
            sha256: version.sha256.clone(),
            vcs_tag: version.vcs_tag.clone().unwrap_or_else(|| "-".to_owned()),
            yanked: version.yanked,
        })
        .collect();
    let graph_versions: Vec<dependency_graph::GraphVersion> = versions
        .iter()
        .map(|version| dependency_graph::GraphVersion {
            version: version.version.clone(),
            prerelease: is_prerelease(&version.version, &version.version_scheme),
            yanked: version.yanked,
        })
        .collect();
    let selected_graph_version = package
        .latest_version
        .as_deref()
        .filter(|latest| {
            versions
                .iter()
                .any(|version| version.version.as_str() == *latest && !version.yanked)
        })
        .or_else(|| {
            versions
                .iter()
                .find(|version| !version.yanked)
                .map(|version| version.version.as_str())
        })
        .or_else(|| versions.first().map(|version| version.version.as_str()));

    let can_manage = viewer.can_administer(&org.slug);
    let can_view_org_topology = viewer.can_see_private(&org.slug);
    // The package entity stores the project UUID, not its URL slug. Resolve the
    // slug only for an authorized org member before rendering the optional
    // project-topology link; public package visitors do not need this private
    // organization listing.
    let project_slug = if can_view_org_topology {
        match package.project_id {
            Some(project_id) => match zed_orm_core::read::project_by_id(db, project_id).await {
                Ok(Some(project)) if project.org_id == org.id => Some(project.slug),
                Ok(_) => None,
                Err(error) => {
                    tracing::warn!(%error, %project_id, "package project lookup failed");
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };

    let content = html! {
        div class="pkg-head" {
            h1 { (org.slug) "/" (package.name) }
            @if package.visibility != "public" {
                span class="badge badge-private" { (package.visibility) }
            }
            @if can_view_org_topology {
                @if let Some(project_slug) = &project_slug {
                    a class="button"
                      href={ "/orgs/" (org.slug) "/projects/" (project_slug) "/dependency-graph" } {
                        "Project graph"
                    }
                }
                a class="button" href={ "/dashboard/" (org.slug) "/dependency-graph" } {
                    "Org graph"
                }
            }
            @if can_manage {
                a class="button"
                  href={ "/orgs/" (org.slug) "/packages/" (package.name) "/settings" } {
                    "Settings"
                }
            }
        }
        @if let Some(description) = &package.description {
            p class="lede" { (description) }
        }

        (components::install_snippet(&org.slug, &package.name))

        dl class="facts" {
            dt { "downloads" } dd { (package.download_count) }
            dt { "versions" } dd { (package.version_count) }
            @if let Some(latest) = &package.latest_version {
                dt { "latest" }
                dd class="mono" {
                    (latest)
                    @if versions
                        .iter()
                        .find(|version| version.version == *latest)
                        .is_some_and(|version| is_prerelease(&version.version, &version.version_scheme)) {
                        " " span class="badge" { "pre-release" }
                    }
                }
            }
            @if !licenses.is_empty() {
                dt { "license" }
                dd {
                    @for license in &licenses {
                        @if license.package_version_id.is_none() {
                            @match license.kind.as_str() {
                                "spdx" => span class="mono" {
                                    (license.spdx_id.clone().unwrap_or_default())
                                },
                                "proprietary" => span { "Proprietary — all rights reserved" },
                                _ => span {
                                    (license.name.clone().unwrap_or_else(|| "Custom".to_owned()))
                                },
                            }
                        }
                    }
                }
            }
            @if is_linkable_url(&package.repo_url) {
                dt { "repository" }
                dd { a href=(package.repo_url) rel="nofollow noopener" { (package.repo_url) } }
            } @else if !package.repo_url.is_empty() {
                dt { "repository" } dd class="mono" { (package.repo_url) }
            }
        }

        @if let Some(selected_version) = selected_graph_version {
            (dependency_graph::package_workspace(
                &org.slug,
                &package.name,
                selected_version,
                &graph_versions,
            ))
        } @else {
            section class="card" id="dependency-graph" {
                h2 { "Dependency graph" }
                p class="muted" { "Publish a version to activate the dependency graph workspace." }
            }
        }

        h2 { "Versions" }
        @if rows.is_empty() {
            p class="muted" { "No versions published yet." }
        } @else {
            (components::version_table(&rows))
        }
    };

    Html(
        layout(
            &format!("{}/{}", org.slug, package.name),
            true,
            &viewer,
            &PageContext::package(&org.slug, &package.name),
            content,
        )
        .into_string(),
    )
    .into_response()
}

fn not_found(state: &WebState, viewer: &Viewer, org: &str, name: &str) -> Response {
    let content = html! {
        h1 { "Package not found" }
        p class="muted" { "No package " span class="mono" { (org) "/" (name) } " is visible to you." }
    };
    (
        StatusCode::NOT_FOUND,
        Html(
            layout(
                "Not found",
                state.db.is_some(),
                viewer,
                &PageContext::none(),
                content,
            )
            .into_string(),
        ),
    )
        .into_response()
}

fn offline(state: &WebState, viewer: &Viewer, org: &str, name: &str) -> Response {
    let content = html! {
        h1 { (org) "/" (name) }
        p class="muted" { "The registry database is unavailable; package details cannot be shown." }
    };
    Html(
        layout(
            &format!("{org}/{name}"),
            state.db.is_some(),
            viewer,
            &PageContext::none(),
            content,
        )
        .into_string(),
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_become_links() {
        assert!(is_linkable_url("https://github.com/zed-pkg/zed-cli"));
        assert!(is_linkable_url("http://example.com"));
        // A stored hostile URL must render as inert text, not an anchor.
        assert!(!is_linkable_url("javascript:alert(1)"));
        assert!(!is_linkable_url("data:text/html,<script>"));
        assert!(!is_linkable_url("git@github.com:zed-pkg/zed-cli.git"));
        assert!(!is_linkable_url(""));
    }

    #[test]
    fn prerelease_detection_respects_the_declared_version_scheme() {
        assert!(is_prerelease("2.0.0-beta.1", "semver"));
        assert!(!is_prerelease("2.0.0", "semver"));
        assert!(is_prerelease("2026.08-preview.1", "calver"));
        assert!(!is_prerelease("release-candidate-1", "opaque"));
    }
}
