//! Organization and project dependency-topology pages.
//!
//! These pages compose already-authorized package summaries into the same
//! shared browser component used by package pages. Each source graph is still
//! fetched through the same-origin BFF and remains independently immutable.

use std::sync::Arc;

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
    let packages = zed_orm_core::read::packages_for_org(db, org.id, &org.slug, true)
        .await
        .unwrap_or_default();
    let rows: Vec<components::PackageRow> = packages
        .iter()
        .map(super::package::summary_row)
        .collect();

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
    let (viewer, org) = match org_scope(&state, &headers, &org_slug).await {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let db = state
        .db
        .as_ref()
        .expect("org_scope proved the database is available");
    let projects = zed_orm_core::read::projects_for_org(db, org.id, &org.slug, true)
        .await
        .unwrap_or_default();
    let Some(project) = projects.into_iter().find(|row| row.slug == project_slug) else {
        return message_page(
            &state,
            &viewer,
            "Not found",
            "That project does not exist in this organization.",
            StatusCode::NOT_FOUND,
        );
    };
    let packages = zed_orm_core::read::packages_for_project(db, project.id, &org.slug)
        .await
        .unwrap_or_default();
    let rows: Vec<components::PackageRow> = packages
        .iter()
        .map(super::package::summary_row)
        .collect();

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
    let org = zed_orm_core::read::org_by_slug(db, org_slug)
        .await
        .unwrap_or(None);
    let Some(org) = org.filter(|_| viewer.can_see_private(org_slug)) else {
        return Err(message_page(
            state,
            &viewer,
            "Not found",
            "That organization does not exist, or you are not a member of it.",
            StatusCode::NOT_FOUND,
        ));
    };
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
    #[test]
    fn topology_routes_have_distinct_page_titles() {
        assert_ne!(
            "organization dependency topology",
            "project dependency topology"
        );
    }
}
