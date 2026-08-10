//! The signed-in console: dashboard and the four settings pages.
//!
//! Every page here is a **read plus forms**. The forms post to the API server
//! (`state.registry_url`), never back to this process — the web tier holds a
//! SELECT-only database identity, so there is no write path here to reach.
//!
//! Authorization is decided once, at the top of each handler, from the resolved
//! session. Templates receive already-decided booleans rather than the viewer,
//! so a template cannot accidentally widen access.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use maud::{Markup, html};

use crate::session::{self, Viewer};
use crate::state::WebState;
use crate::views::{PageContext, components, layout};

/// `/orgs/{org}` is the org's identity, not a page; the dashboard is.
pub async fn org_redirect(Path(org): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/dashboard/{org}"))
}

/// Shared preamble: resolve the viewer, require the database, require the org,
/// and require membership. Returns the rendered rejection when any step fails.
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

    // A non-member gets "not found" rather than "forbidden": confirming that a
    // private org exists is itself a disclosure.
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

// ─────────────────────────────────────────────────────────────────────────────
// /dashboard/{org}
// ─────────────────────────────────────────────────────────────────────────────

pub async fn dashboard(
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
        .expect("org_scope proved the database is up");

    let projects = zed_orm_core::read::projects_for_org(db, org.id, &org.slug, true)
        .await
        .unwrap_or_default();
    let packages = zed_orm_core::read::packages_for_org(db, org.id, &org.slug, true)
        .await
        .unwrap_or_default();
    let can_manage = viewer.can_administer(&org.slug);

    let rows: Vec<components::PackageRow> =
        packages.iter().map(super::package::summary_row).collect();

    let content = html! {
        div class="pkg-head" {
            h1 { (org.name) }
            span class="badge" { (viewer.role_in(&org.slug).unwrap_or("member")) }
            @if can_manage {
                a class="button" href={ "/orgs/" (org.slug) "/settings" } { "Org settings" }
            }
        }
        @if let Some(description) = &org.description {
            p class="lede" { (description) }
        }

        h2 { "Projects" }
        @if projects.is_empty() {
            p class="muted" {
                "No projects yet."
                @if can_manage {
                    " " a href={ "/orgs/" (org.slug) "/settings#new-project" } { "Create one" } "."
                }
            }
        } @else {
            ul class="org-list" {
                @for project in &projects {
                    li {
                        a class="org-name"
                          href={ "/orgs/" (org.slug) "/projects/" (project.slug) "/settings" } {
                            (project.name)
                        }
                        @if let Some(description) = &project.description {
                            span class="pkg-desc" { (description) }
                        }
                    }
                }
            }
        }

        h2 { "Packages" }
        (components::package_rows(&rows, "No packages yet."))
    };

    Html(
        layout(
            &org.name,
            true,
            &viewer,
            &PageContext::org(&org.slug),
            content,
        )
        .into_string(),
    )
    .into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// /orgs/{org}/settings
// ─────────────────────────────────────────────────────────────────────────────

pub async fn org_settings(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(org_slug): Path<String>,
) -> Response {
    let (viewer, org) = match org_scope(&state, &headers, &org_slug).await {
        Ok(scope) => scope,
        Err(response) => return response,
    };

    if !viewer.can_administer(&org.slug) {
        return message_page(
            &state,
            &viewer,
            "Not permitted",
            "Only an owner or admin can change organization settings.",
            StatusCode::FORBIDDEN,
        );
    }

    let api = &state.registry_url;
    let content = html! {
        h1 { "Settings — " (org.name) }

        section class="card" {
            h2 { "Members" }
            form class="stack" method="post"
                 action={ (api) "/v1/orgs/" (org.slug) "/invitations" } {
                label for="invite-email" { "Invite by email" }
                input id="invite-email" type="email" name="email" required
                      placeholder="colleague@example.com";
                label for="invite-role" { "Role" }
                select id="invite-role" name="role" {
                    option value="admin" { "Admin — manage settings and members" }
                    option value="member" selected { "Member — publish and manage packages" }
                    option value="reader" { "Reader — read private packages" }
                }
                button type="submit" class="button primary" { "Send invitation" }
            }
        }

        section class="card" id="new-project" {
            h2 { "New project" }
            p class="muted" {
                "A project groups packages inside this organization. Package names stay "
                "unique per organization, so filing a package under a project never changes its URL."
            }
            form class="stack" method="post" action={ (api) "/v1/orgs/" (org.slug) "/projects" } {
                label for="project-slug" { "Slug" }
                input id="project-slug" name="slug" required pattern="[a-z0-9][a-z0-9._-]*[a-z0-9]"
                      placeholder="platform";
                label for="project-name" { "Name" }
                input id="project-name" name="name" required placeholder="Platform";
                label for="project-description" { "Description" }
                input id="project-description" name="description" placeholder="optional";
                button type="submit" class="button primary" { "Create project" }
            }
        }

        section class="card" id="new-package" {
            h2 { "New package" }
            p class="muted" { "New packages start private." }
            form class="stack" method="post" action={ (api) "/v1/orgs/" (org.slug) "/packages" } {
                label for="package-name" { "Name" }
                input id="package-name" name="name" required pattern="[a-z0-9][a-z0-9._-]*[a-z0-9]"
                      placeholder="http-client";
                label for="package-description" { "Description" }
                input id="package-description" name="description" placeholder="optional";
                button type="submit" class="button primary" { "Create package" }
            }
        }

        section class="card" id="new-org" {
            h2 { "New organization" }
            form class="stack" method="post" action={ (api) "/v1/orgs" } {
                label for="org-slug" { "Slug" }
                input id="org-slug" name="slug" required pattern="[a-z0-9][a-z0-9-]*[a-z0-9]";
                label for="org-name" { "Name" }
                input id="org-name" name="name" required;
                button type="submit" class="button primary" { "Create organization" }
            }
        }
    };

    Html(
        layout(
            &format!("Settings — {}", org.name),
            true,
            &viewer,
            &PageContext::org(&org.slug),
            content,
        )
        .into_string(),
    )
    .into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// /orgs/{org}/projects/{project}/settings
// ─────────────────────────────────────────────────────────────────────────────

pub async fn project_settings(
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
        .expect("org_scope proved the database is up");

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
    let rows: Vec<components::PackageRow> =
        packages.iter().map(super::package::summary_row).collect();
    let can_manage = viewer.can_administer(&org.slug);
    let api = &state.registry_url;

    let content = html! {
        h1 { "Settings — " (project.name) }
        p class="muted" { "Project in " a href={ "/dashboard/" (org.slug) } { (org.name) } }

        h2 { "Packages" }
        (components::package_rows(&rows, "No packages filed under this project yet."))

        @if can_manage {
            section class="card" {
                h2 { "Invite to this project" }
                form class="stack" method="post"
                     action={ (api) "/v1/projects/" (project.id) "/invitations" } {
                    label for="project-invite-email" { "Email" }
                    input id="project-invite-email" type="email" name="email" required;
                    label for="project-invite-role" { "Role" }
                    select id="project-invite-role" name="role" {
                        option value="admin" { "Admin" }
                        option value="member" selected { "Member" }
                        option value="reader" { "Reader" }
                    }
                    button type="submit" class="button primary" { "Send invitation" }
                }
            }

            section class="card" id="add-package" {
                h2 { "Add a package" }
                form class="stack" method="post"
                     action={ (api) "/v1/orgs/" (org.slug) "/packages" } {
                    input type="hidden" name="project_id" value=(project.id);
                    label for="new-package-name" { "Name" }
                    input id="new-package-name" name="name" required
                          pattern="[a-z0-9][a-z0-9._-]*[a-z0-9]";
                    button type="submit" class="button primary" { "Create package" }
                }
            }
        }
    };

    Html(
        layout(
            &format!("Settings — {}", project.name),
            true,
            &viewer,
            &PageContext::project(&org.slug, &project.slug, &project.name),
            content,
        )
        .into_string(),
    )
    .into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// /orgs/{org}/packages/{name}/settings
// ─────────────────────────────────────────────────────────────────────────────

pub async fn package_settings(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path((org_slug, name)): Path<(String, String)>,
) -> Response {
    let (viewer, org) = match org_scope(&state, &headers, &org_slug).await {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let db = state
        .db
        .as_ref()
        .expect("org_scope proved the database is up");

    if !viewer.can_administer(&org.slug) {
        return message_page(
            &state,
            &viewer,
            "Not permitted",
            "Only an owner or admin can configure a package.",
            StatusCode::FORBIDDEN,
        );
    }

    let Some((package, _)) = zed_orm_core::read::package_by_org_and_name(db, &org.slug, &name)
        .await
        .unwrap_or(None)
    else {
        return message_page(
            &state,
            &viewer,
            "Not found",
            "That package does not exist in this organization.",
            StatusCode::NOT_FOUND,
        );
    };

    let limits = zed_orm_core::read::visibility_limits(db).await.ok();
    let api = &state.registry_url;

    let content = html! {
        h1 { "Settings — " (org.slug) "/" (package.name) }
        p class="muted" { a href={ "/p/" (org.slug) "/" (package.name) } { "View public page" } }

        section class="card" {
            h2 { "Details" }
            form class="stack" method="post"
                 action={ (api) "/v1/orgs/" (org.slug) "/packages/" (package.name) } {
                label for="pkg-description" { "Description" }
                input id="pkg-description" name="description"
                      value=(package.description.clone().unwrap_or_default());
                label for="pkg-repo" { "Repository URL" }
                input id="pkg-repo" name="repo_url" value=(package.repo_url);
                button type="submit" class="button primary" { "Save" }
            }
        }

        (visibility_card(&package, limits.as_ref(), api, &org.slug))

        section class="card" {
            h2 { "Download" }
            p class="muted" { "Export the latest version's artifact." }
            div class="row" {
                a class="button"
                  href={ (api) "/v1/orgs/" (org.slug) "/packages/" (package.name)
                         "/archive?format=tar.gz" } { "Download .tar.gz" }
                a class="button"
                  href={ (api) "/v1/orgs/" (org.slug) "/packages/" (package.name)
                         "/archive?format=zip" } { "Download .zip" }
            }
        }
    };

    Html(
        layout(
            &format!("Settings — {}", package.name),
            true,
            &viewer,
            &PageContext::package(&org.slug, &package.name),
            content,
        )
        .into_string(),
    )
    .into_response()
}

/// The visibility control, which is the one place the promotion rule surfaces
/// to a user.
///
/// When a private package has already left the window, the control is disabled
/// and says exactly which limit closed it — rather than offering a button that
/// the database will reject with `ZD001`/`ZD002`.
fn visibility_card(
    package: &zed_orm_core::entities::package::Model,
    limits: Option<&zed_orm_core::VisibilityLimits>,
    api: &str,
    org_slug: &str,
) -> Markup {
    let is_public = package.visibility == "public";
    let refusal = limits.and_then(|limits| {
        let age_days = (chrono::Utc::now().fixed_offset() - package.created_at).num_seconds()
            as f64
            / 86_400.0;
        limits.evaluate(age_days, package.download_count)
    });

    html! {
        section class="card" {
            h2 { "Visibility" }
            p {
                "This package is " span class="badge" { (package.visibility) } "."
            }
            @if is_public {
                p class="muted" { "Public packages are visible to everyone and installable anonymously." }
            } @else if let Some(refusal) = refusal {
                p class="muted warn" { (refusal.to_string()) }
                p class="muted" {
                    "A package can only be made public early in its life, so that making it "
                    "public cannot quietly expose something already established and depended on."
                }
                button class="button" disabled { "Make public" }
            } @else {
                @if let Some(limits) = limits {
                    p class="muted" {
                        "You can still make this package public: the window is the first "
                        (limits.max_age_days()) " days and up to "
                        (limits.max_downloads()) " downloads."
                    }
                }
                form method="post"
                     action={ (api) "/v1/orgs/" (org_slug) "/packages/" (package.name) "/visibility" } {
                    input type="hidden" name="visibility" value="public";
                    button type="submit" class="button primary" { "Make public" }
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// /settings
// ─────────────────────────────────────────────────────────────────────────────

pub async fn user_settings(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let viewer = session::resolve(&state, &headers).await;

    let Some(user) = viewer.user().cloned() else {
        return Redirect::temporary("/shared-auth/auth/browser/sign-in").into_response();
    };

    let api = &state.registry_url;
    let content = html! {
        h1 { "Your settings" }

        section class="card" {
            h2 { "Profile" }
            p class="muted" {
                "Your email and sign-in method are managed by your identity provider, "
                "not here."
            }
            form class="stack" method="post" action={ (api) "/v1/users/me" } {
                label for="display-name" { "Display name" }
                input id="display-name" name="display_name"
                      value=(user.display_name.clone().unwrap_or_default());
                label for="avatar-url" { "Avatar URL" }
                input id="avatar-url" name="avatar_url"
                      value=(user.avatar_url.clone().unwrap_or_default());
                button type="submit" class="button primary" { "Save" }
            }
        }

        section class="card" {
            h2 { "Organizations" }
            @if viewer.orgs().is_empty() {
                p class="muted" { "You are not a member of any organization yet." }
            } @else {
                ul class="org-list" {
                    @for org in viewer.orgs() {
                        li {
                            a class="org-name" href={ "/dashboard/" (org.slug) } { (org.name) }
                            span class="badge" { (org.role) }
                        }
                    }
                }
            }
        }

        section class="card" id="new-org" {
            h2 { "New organization" }
            form class="stack" method="post" action={ (api) "/v1/orgs" } {
                label for="user-org-slug" { "Slug" }
                input id="user-org-slug" name="slug" required pattern="[a-z0-9][a-z0-9-]*[a-z0-9]";
                label for="user-org-name" { "Name" }
                input id="user-org-name" name="name" required;
                button type="submit" class="button primary" { "Create organization" }
            }
        }
    };

    Html(
        layout(
            "Your settings",
            state.db.is_some(),
            &viewer,
            &PageContext::none(),
            content,
        )
        .into_string(),
    )
    .into_response()
}
