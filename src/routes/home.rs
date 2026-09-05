//! `/` — the home page.
//!
//! Signed in: the viewer's orgs, their projects, and a search box scoped to
//! everything they can see. Signed out: the newest public packages.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Html;
use maud::html;

use crate::session::{self, Viewer};
use crate::state::WebState;
use crate::views::{PageContext, components, layout};

pub async fn page(State(state): State<Arc<WebState>>, headers: HeaderMap) -> Html<String> {
    let viewer = session::resolve(&state, &headers).await;

    let content = match &viewer {
        Viewer::SignedIn(signed_in) => {
            let projects = match &state.db {
                Some(db) => zed_orm_core::read::projects_for_user(db, signed_in.user.id)
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            html! {
                h1 { "Your organizations" }
                @if signed_in.orgs.is_empty() {
                    p class="muted" {
                        "You are not a member of any organization yet. "
                        a href="/onboarding" { "Set up your workspace" } "."
                    }
                } @else {
                    ul class="org-list" {
                        @for org in &signed_in.orgs {
                            li {
                                a class="org-name" href={ "/dashboard/" (org.slug) } { (org.name) }
                                span class="badge" { (org.role) }
                                @if let Some(description) = &org.description {
                                    span class="pkg-desc" { (description) }
                                }
                            }
                        }
                    }
                }

                @if !projects.is_empty() {
                    h2 { "Your projects" }
                    ul class="org-list" {
                        @for project in &projects {
                            li {
                                a class="org-name"
                                  href={ "/orgs/" (project.org_slug) "/projects/" (project.slug) "/settings" } {
                                    (project.org_slug) "/" (project.slug)
                                }
                                @if let Some(description) = &project.description {
                                    span class="pkg-desc" { (description) }
                                }
                            }
                        }
                    }
                }

                h2 { "Search" }
                (components::search_box(""))
            }
        }
        Viewer::Anonymous => {
            let recent = match &state.db {
                Some(db) => zed_orm_core::read::recent_public_packages(db, 20)
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let rows: Vec<components::PackageRow> =
                recent.iter().map(super::package::summary_row).collect();
            html! {
                h1 { "zed-pkg registry" }
                p class="muted" {
                    "A universal package manager backed by git and mercurial servers. "
                    a href="/shared-auth/auth/browser/sign-in" { "Sign in" }
                    " to manage your organizations."
                }
                h2 { "Recently published" }
                (components::package_rows(&rows, "No public packages yet."))
                h2 { "Search" }
                (components::search_box(""))
            }
        }
    };

    Html(
        layout(
            "Home",
            state.db.is_some(),
            &viewer,
            &PageContext::none(),
            content,
        )
        .into_string(),
    )
}
