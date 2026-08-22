//! Organization and project dependency-topology pages.
//!
//! These pages compose already-authorized package summaries into the same
//! shared browser component used by package pages. Each source graph is still
//! fetched through the same-origin BFF and remains independently immutable.

use std::{collections::HashMap, sync::Arc};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use maud::html;

use crate::session::{self, Viewer};
use crate::state::WebState;
use crate::views::{PageContext, components, dependency_graph, layout};

pub async fn organization(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org_slug): Path<String>,
) -> Response {
    let (viewer, org) = match org_scope(&state, &headers, &org_slug).await {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let db = state
        .db
        .as_ref()
        .expect("org_scope proved the database is available");
    let packages = match zed_orm_core::read::packages_for_org(db, org.id, &org.slug, true).await {
        Ok(packages) => packages,
        Err(error) => {
            tracing::warn!(%error, org = %org.slug, "organization graph package lookup failed");
            return message_page(
                &state,
                &viewer,
                "Topology unavailable",
                "The organization package topology is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
    };
    let rows = match latest_graph_rows(db, &packages).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, org = %org.slug, "organization graph latest-version lookup failed");
            return message_page(
                &state,
                &viewer,
                "Topology unavailable",
                "The organization package topology is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
    };

    let content = html! {
        div class="pkg-head" {
            div {
                p class="dg-eyebrow" { "Organization intelligence" }
                h1 { (org.name) " dependency topology" }
            }
            a class="button" href={ "/dashboard/" (org.slug) } { "Back to organization" }
        }
        p class="lede" {
            "This view composes each package's latest declared graph. Select a node to inspect impact, "
            "open the package, or expand beyond the organization boundary."
        }
        (dependency_graph::scope_workspace(
            "organization",
            &format!("{} package topology", org.name),
            "Compare relationships across every published package visible in this organization.",
            &rows,
        ))
    };

    Html(
        layout(
            &format!("{} dependency topology", org.name),
            true,
            &viewer,
            &PageContext::org(&org.slug),
            content,
        )
        .into_string(),
    )
    .into_response()
}

pub async fn project(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org_slug, project_slug)): Path<(String, String)>,
) -> Response {
    let (viewer, org, project) =
        match project_scope(&state, &headers, &org_slug, &project_slug).await {
            Ok(scope) => scope,
            Err(response) => return response,
        };
    let db = state
        .db
        .as_ref()
        .expect("project_scope proved the database is available");
    let packages = match zed_orm_core::read::packages_for_project(db, project.id, &org.slug).await {
        Ok(packages) => packages,
        Err(error) => {
            tracing::warn!(
                %error,
                org = %org.slug,
                project = %project.slug,
                "project graph package lookup failed"
            );
            return message_page(
                &state,
                &viewer,
                "Topology unavailable",
                "The project package topology is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
    };
    let rows = match latest_graph_rows(db, &packages).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(
                %error,
                org = %org.slug,
                project = %project.slug,
                "project graph latest-version lookup failed"
            );
            return message_page(
                &state,
                &viewer,
                "Topology unavailable",
                "The project package topology is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
    };

    let content = html! {
        div class="pkg-head" {
            div {
                p class="dg-eyebrow" { "Project intelligence" }
                h1 { (project.name) " dependency topology" }
            }
            a class="button"
              href={ "/orgs/" (org.slug) "/projects/" (project.slug) "/settings" } {
                "Back to project"
            }
        }
        @if let Some(description) = &project.description {
            p class="lede" { (description) }
        }
        (dependency_graph::scope_workspace(
            "project",
            &format!("{} package topology", project.name),
            "Trace dependencies and reverse impact among the latest published packages assigned to this project.",
            &rows,
        ))
    };

    Html(
        layout(
            &format!("{} dependency topology", project.name),
            true,
            &viewer,
            &PageContext::project(&org.slug, &project.slug, &project.name),
            content,
        )
        .into_string(),
    )
    .into_response()
}

async fn latest_graph_rows(
    db: &zed_orm_core::ReadContext,
    packages: &[zed_orm_core::models::PackageSummary],
) -> Result<Vec<components::PackageRow>, zed_orm_core::OrmError> {
    let coordinates = packages
        .iter()
        .filter_map(|package| {
            package
                .latest_version
                .as_ref()
                .map(|version| (package.id, version.clone()))
        })
        .collect::<Vec<_>>();
    let versions =
        zed_orm_core::version_reads::exact_unyanked_package_versions(db, &coordinates).await?;
    let license_coordinates = versions
        .iter()
        .map(|version| (version.package_id, version.id))
        .collect::<Vec<_>>();
    let licenses =
        zed_orm_core::read::primary_licenses_for_exact_versions(db, &license_coordinates).await?;
    let versions_by_package = versions
        .into_iter()
        .map(|version| {
            let prerelease = is_prerelease(&version.version, &version.version_scheme);
            (
                version.package_id,
                (version.id, version.version, prerelease),
            )
        })
        .collect::<HashMap<_, _>>();
    let licenses_by_package = effective_license_labels(&versions_by_package, licenses);

    Ok(latest_graph_rows_from_versions(
        packages,
        &versions_by_package,
        &licenses_by_package,
    ))
}

fn latest_graph_rows_from_versions(
    packages: &[zed_orm_core::models::PackageSummary],
    versions_by_package: &HashMap<uuid::Uuid, (uuid::Uuid, String, bool)>,
    licenses_by_package: &HashMap<uuid::Uuid, String>,
) -> Vec<components::PackageRow> {
    packages
        .iter()
        .map(|package| {
            let mut row = super::package::summary_row(package);
            let latest = package
                .latest_version
                .as_ref()
                .and_then(|_| versions_by_package.get(&package.id));
            row.latest = latest.map(|(_, version, _)| version.clone());
            row.latest_prerelease = latest.is_some_and(|(_, _, prerelease)| *prerelease);
            row.latest_license = latest
                .and_then(|_| licenses_by_package.get(&package.id))
                .cloned();
            row
        })
        .collect()
}

fn effective_license_labels(
    versions_by_package: &HashMap<uuid::Uuid, (uuid::Uuid, String, bool)>,
    licenses: Vec<zed_orm_core::entities::package_license::Model>,
) -> HashMap<uuid::Uuid, String> {
    let mut defaults = HashMap::new();
    let mut overrides = HashMap::new();
    for license in licenses {
        let Some((version_id, _, _)) = versions_by_package.get(&license.package_id) else {
            continue;
        };
        let label = license.spdx_id.or(license.name).unwrap_or(license.kind);
        match license.package_version_id {
            Some(candidate) if candidate == *version_id => {
                overrides.insert(license.package_id, label);
            }
            None => {
                defaults.entry(license.package_id).or_insert(label);
            }
            Some(_) => {}
        }
    }
    defaults.extend(overrides);
    defaults
}

fn is_prerelease(version: &str, scheme: &str) -> bool {
    let scheme = zed_interfaces::version::VersionScheme::from_str_lenient(scheme);
    if scheme == zed_interfaces::version::VersionScheme::Opaque {
        return false;
    }
    zed_interfaces::version::parse_version(version).is_some_and(|parsed| !parsed.pre.is_empty())
}

#[allow(clippy::result_large_err)] // Scope failures are rendered Axum responses.
async fn project_scope(
    state: &WebState,
    headers: &HeaderMap,
    org_slug: &str,
    project_slug: &str,
) -> Result<
    (
        Viewer,
        zed_orm_core::entities::org::Model,
        zed_orm_core::models::ProjectSummary,
    ),
    Response,
> {
    let viewer = session::resolve(state, headers).await;
    let Some(db) = &state.db else {
        return Err(message_page(
            state,
            &viewer,
            "Registry offline",
            "The registry database is unavailable.",
            StatusCode::SERVICE_UNAVAILABLE,
        ));
    };
    let org = match zed_orm_core::read::org_by_slug(db, org_slug).await {
        Ok(Some(org)) => org,
        Ok(None) => return Err(project_not_found(state, &viewer)),
        Err(error) => {
            tracing::warn!(%error, org = org_slug, "project graph organization lookup failed");
            return Err(message_page(
                state,
                &viewer,
                "Registry unavailable",
                "Dependency topology metadata is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }
    };
    let project = match zed_orm_core::read::project_by_org_and_slug(db, org.id, project_slug).await
    {
        Ok(Some(project)) => project,
        Ok(None) => return Err(project_not_found(state, &viewer)),
        Err(error) => {
            tracing::warn!(%error, org = org_slug, project = project_slug, "project graph lookup failed");
            return Err(message_page(
                state,
                &viewer,
                "Topology unavailable",
                "The project topology is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }
    };

    let org_role = match session::exact_org_role(db, &viewer, org.id).await {
        Ok(role) => role,
        Err(error) => {
            tracing::warn!(%error, org = org_slug, project = project_slug, "project graph organization membership lookup failed");
            return Err(message_page(
                state,
                &viewer,
                "Topology unavailable",
                "The project topology is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }
    };
    let direct_role = if org_role.is_some() {
        None
    } else if viewer.user().is_some() {
        match session::exact_project_role(db, &viewer, project.id).await {
            Ok(Some(role)) => Some(role),
            Ok(None) => return Err(project_not_found(state, &viewer)),
            Err(error) => {
                tracing::warn!(%error, org = org_slug, project = project_slug, "project graph membership lookup failed");
                return Err(message_page(
                    state,
                    &viewer,
                    "Topology unavailable",
                    "The project topology is temporarily unavailable.",
                    StatusCode::SERVICE_UNAVAILABLE,
                ));
            }
        }
    } else {
        return Err(project_not_found(state, &viewer));
    };

    let summary = zed_orm_core::models::ProjectSummary {
        id: project.id,
        org_id: project.org_id,
        org_slug: org.slug.clone(),
        slug: project.slug,
        name: project.name,
        description: project.description,
        role: direct_role.or(org_role).unwrap_or_default(),
    };
    Ok((viewer, org, summary))
}

fn project_not_found(state: &WebState, viewer: &Viewer) -> Response {
    message_page(
        state,
        viewer,
        "Not found",
        "That project does not exist in this organization.",
        StatusCode::NOT_FOUND,
    )
}

#[allow(clippy::result_large_err)] // Scope failures are rendered Axum responses.
async fn org_scope(
    state: &WebState,
    headers: &HeaderMap,
    org_slug: &str,
) -> Result<(Viewer, zed_orm_core::entities::org::Model), Response> {
    let viewer = session::resolve(state, headers).await;
    let Some(db) = &state.db else {
        return Err(message_page(
            state,
            &viewer,
            "Registry offline",
            "The registry database is unavailable.",
            StatusCode::SERVICE_UNAVAILABLE,
        ));
    };
    let org = match zed_orm_core::read::org_by_slug(db, org_slug).await {
        Ok(org) => org,
        Err(error) => {
            tracing::warn!(%error, org = org_slug, "graph organization lookup failed");
            return Err(message_page(
                state,
                &viewer,
                "Registry unavailable",
                "Dependency topology metadata is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }
    };
    let Some(org) = org else {
        return Err(message_page(
            state,
            &viewer,
            "Not found",
            "That organization does not exist, or you are not a member of it.",
            StatusCode::NOT_FOUND,
        ));
    };
    let role = match session::exact_org_role(db, &viewer, org.id).await {
        Ok(role) => role,
        Err(error) => {
            tracing::warn!(%error, org = org_slug, "graph organization membership lookup failed");
            return Err(message_page(
                state,
                &viewer,
                "Registry unavailable",
                "Dependency topology metadata is temporarily unavailable.",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }
    };
    if role.is_none() {
        return Err(message_page(
            state,
            &viewer,
            "Not found",
            "That organization does not exist, or you are not a member of it.",
            StatusCode::NOT_FOUND,
        ));
    }
    Ok((viewer, org))
}

fn message_page(
    state: &WebState,
    viewer: &Viewer,
    title: &str,
    body: &str,
    status: StatusCode,
) -> Response {
    let content = html! {
        h1 { (title) }
        p class="muted" { (body) }
    };
    (
        status,
        Html(
            layout(
                title,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;
    use uuid::Uuid;
    use zed_orm_core::models::PackageSummary;

    use super::{effective_license_labels, is_prerelease, latest_graph_rows_from_versions};

    fn package_summary(id: Uuid, name: &str, latest_version: Option<&str>) -> PackageSummary {
        PackageSummary {
            id,
            org_id: Uuid::from_u128(100),
            org_slug: "acme".into(),
            project_id: None,
            project_slug: None,
            name: name.into(),
            description: None,
            visibility: "public".into(),
            repo_url: format!("https://example.test/{name}"),
            config: json!({}),
            updated_at: chrono::DateTime::parse_from_rfc3339("2026-08-22T00:00:00Z").unwrap(),
            latest_version: latest_version.map(str::to_owned),
            download_count: 0,
            version_count: i32::from(latest_version.is_some()),
        }
    }

    fn primary_license(
        package_id: Uuid,
        package_version_id: Option<Uuid>,
        spdx_id: &str,
    ) -> zed_orm_core::entities::package_license::Model {
        let timestamp = chrono::DateTime::parse_from_rfc3339("2026-08-22T00:00:00Z").unwrap();
        zed_orm_core::entities::package_license::Model {
            id: Uuid::new_v4(),
            package_id,
            package_version_id,
            kind: "spdx".into(),
            spdx_id: Some(spdx_id.into()),
            name: None,
            url: None,
            text_body: None,
            is_primary: true,
            created_at: timestamp,
            updated_at: timestamp,
        }
    }

    #[test]
    fn topology_routes_have_distinct_page_titles() {
        assert_ne!(
            "organization dependency topology",
            "project dependency topology"
        );
    }

    #[test]
    fn batched_latest_versions_preserve_package_order_and_missing_rows() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let unpublished = Uuid::from_u128(3);
        let packages = vec![
            package_summary(first, "first", Some("1.0.0")),
            package_summary(second, "second", Some("2.0.0")),
            package_summary(unpublished, "unpublished", None),
        ];
        let versions = HashMap::from([
            (second, (Uuid::from_u128(20), "2.0.0".to_owned(), false)),
            (
                unpublished,
                (Uuid::from_u128(30), "9.9.9".to_owned(), false),
            ),
        ]);

        let rows = latest_graph_rows_from_versions(&packages, &versions, &HashMap::new());

        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["first", "second", "unpublished"]
        );
        assert_eq!(rows[0].latest, None);
        assert_eq!(rows[1].latest.as_deref(), Some("2.0.0"));
        assert_eq!(rows[2].latest, None);
    }

    #[test]
    fn topology_channel_metadata_respects_the_declared_version_scheme() {
        assert!(is_prerelease("2.0.0-beta.1", "semver"));
        assert!(is_prerelease("2026.08-preview.1", "calver"));
        assert!(!is_prerelease("release-candidate-1", "opaque"));
    }

    #[test]
    fn exact_version_license_overrides_the_package_default() {
        let package_id = Uuid::from_u128(1);
        let version_id = Uuid::from_u128(2);
        let versions = HashMap::from([(package_id, (version_id, "1.0.0".to_owned(), false))]);
        let licenses = vec![
            primary_license(package_id, None, "Apache-2.0"),
            primary_license(package_id, Some(version_id), "MIT"),
            primary_license(package_id, Some(Uuid::from_u128(3)), "BSD-3-Clause"),
        ];

        assert_eq!(
            effective_license_labels(&versions, licenses).get(&package_id),
            Some(&"MIT".to_owned())
        );
    }
}
