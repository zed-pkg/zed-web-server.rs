//! The page shell and its context-aware header.
//!
//! The header is *static in structure and context-aware in content*: every page
//! renders the same skeleton — brand, breadcrumb, search, create menu, account
//! menu — and the current route plus the viewer fill it in. Pages never
//! assemble their own navigation, so a new page cannot accidentally ship
//! without the org switcher or with a "create" menu it should not have.

use maud::{DOCTYPE, Markup, html};

use crate::session::Viewer;

/// Where the viewer currently is. Drives the breadcrumb and which entries the
/// create menu offers.
#[derive(Debug, Clone, Default)]
pub struct PageContext {
    pub org: Option<String>,
    pub project: Option<(String, String)>,
    pub package: Option<String>,
}

impl PageContext {
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn org(slug: &str) -> Self {
        Self {
            org: Some(slug.to_owned()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn project(org: &str, slug: &str, name: &str) -> Self {
        Self {
            org: Some(org.to_owned()),
            project: Some((slug.to_owned(), name.to_owned())),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn package(org: &str, name: &str) -> Self {
        Self {
            org: Some(org.to_owned()),
            package: Some(name.to_owned()),
            ..Self::default()
        }
    }
}

/// Render a full page.
pub fn layout(
    title: &str,
    db_online: bool,
    viewer: &Viewer,
    context: &PageContext,
    content: Markup,
) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="htmx-config"
                    content=r#"{"allowEval":false,"allowScriptTags":false,"includeIndicatorStyles":false,"selfRequestsOnly":true}"#;
                title { (title) " · zed-pkg" }
                link rel="stylesheet" href="/static/styles.css";
                link rel="stylesheet" href="/graph-assets/dependency-graph.css";
                link rel="stylesheet" href="/static/dependency-graph-insights.css";
                link rel="icon" type="image/svg+xml" href="/static/favicon.svg";
                script src="/static/htmx.min.js" {}
                script type="module" src="/graph-assets/dependency-graph.js" {}
                script type="module" src="/static/dependency-graph-insights.js" {}
            }
            body {
                (header(viewer, context))
                @if !db_online {
                    div class="banner offline" {
                        "registry offline: no database connection; showing empty state"
                    }
                }
                main class="wrap" { (content) }
                footer {
                    div class="wrap" {
                        "(c) 2026 zed-pkg contributors - MIT - MASH stack (Maud, Axum, SeaORM, HTMX)"
                    }
                }
            }
        }
    }
}

fn header(viewer: &Viewer, context: &PageContext) -> Markup {
    html! {
        nav {
            div class="wrap nav-inner" {
                a class="brand" href="/" {
                    span class="brand-z" { "zed" } span class="brand-pkg" { "-pkg" }
                }
                (breadcrumb(context))
                form class="nav-search" action="/search" method="get" role="search" {
                    input
                        class="search search-compact"
                        type="search"
                        name="q"
                        placeholder="search packages"
                        aria-label="search packages";
                }
                div class="nav-links" {
                    @if viewer.is_signed_in() {
                        (create_menu(viewer, context))
                        (account_menu(viewer))
                    } @else {
                        a class="nav-cta" href="/shared-auth/auth/browser/sign-in" { "Sign in" }
                    }
                }
            }
        }
    }
}

/// Org / project / package trail, with the org switcher hanging off the org.
fn breadcrumb(context: &PageContext) -> Markup {
    html! {
        div class="crumbs" {
            @if let Some(org) = &context.org {
                a class="crumb" href={ "/dashboard/" (org) } { (org) }
                @if let Some((slug, _name)) = &context.project {
                    span class="crumb-sep" { "/" }
                    a class="crumb" href={ "/orgs/" (org) "/projects/" (slug) "/settings" } { (slug) }
                }
                @if let Some(package) = &context.package {
                    span class="crumb-sep" { "/" }
                    a class="crumb" href={ "/p/" (org) "/" (package) } { (package) }
                }
            }
        }
    }
}

/// The create menu. Org creation is always available to a signed-in user;
/// project and package creation appear only inside an org the viewer may
/// administer, so the menu never offers an action that would 403.
fn create_menu(viewer: &Viewer, context: &PageContext) -> Markup {
    let org = context.org.as_deref();
    let may_create_in_org = org.is_some_and(|slug| viewer.can_administer(slug));

    html! {
        details class="menu" {
            summary class="menu-button" { "Create" span class="caret" { "▾" } }
            div class="menu-panel" {
                @if let (Some(slug), true) = (org, may_create_in_org) {
                    a href={ "/orgs/" (slug) "/settings#new-project" } { "New project" }
                    a href={ "/orgs/" (slug) "/settings#new-package" } { "New package" }
                    div class="menu-sep" {}
                }
                a href="/settings#new-org" { "New organization" }
            }
        }
    }
}

fn account_menu(viewer: &Viewer) -> Markup {
    let label = viewer
        .user()
        .map(|user| {
            user.display_name
                .clone()
                .or_else(|| user.email.clone())
                .unwrap_or_else(|| "Account".to_owned())
        })
        .unwrap_or_else(|| "Account".to_owned());

    html! {
        details class="menu" {
            summary class="menu-button" {
                @if let Some(url) = viewer.user().and_then(|user| user.avatar_url.as_deref()) {
                    img class="avatar" src=(url) alt="";
                }
                (label) span class="caret" { "▾" }
            }
            div class="menu-panel" {
                @if viewer.orgs().is_empty() {
                    span class="menu-empty" { "No organizations yet" }
                } @else {
                    span class="menu-label" { "Organizations" }
                    @for org in viewer.orgs() {
                        a href={ "/dashboard/" (org.slug) } {
                            (org.name) span class="menu-role" { (org.role) }
                        }
                    }
                }
                div class="menu-sep" {}
                a href="/settings" { "User settings" }
                form method="post" action="/shared-auth/auth/logout" {
                    button type="submit" class="menu-signout" { "Sign out" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SignedInViewer, Viewer};
    use zed_orm_core::models::{OrgSummary, UserSummary};

    fn signed_in(role: &str) -> Viewer {
        Viewer::SignedIn(Box::new(SignedInViewer {
            user: UserSummary {
                id: uuid::Uuid::nil(),
                subject: uuid::Uuid::nil(),
                realm: "customer".into(),
                email: Some("a@example.com".into()),
                display_name: Some("Ada".into()),
                avatar_url: None,
                settings: serde_json::Value::Object(Default::default()),
            },
            orgs: vec![OrgSummary {
                id: uuid::Uuid::nil(),
                slug: "acme".into(),
                name: "Acme".into(),
                description: None,
                role: role.into(),
            }],
        }))
    }

    #[test]
    fn anonymous_header_offers_sign_in_and_no_menus() {
        let markup = layout(
            "t",
            true,
            &Viewer::Anonymous,
            &PageContext::none(),
            html! {},
        )
        .into_string();
        assert!(markup.contains("Sign in"));
        assert!(!markup.contains("menu-button"));
    }

    #[test]
    fn signed_in_header_lists_orgs_and_the_account_menu() {
        let markup = layout(
            "t",
            true,
            &signed_in("owner"),
            &PageContext::none(),
            html! {},
        )
        .into_string();
        assert!(markup.contains("Ada"));
        assert!(markup.contains("/dashboard/acme"));
        assert!(!markup.contains("Sign in"));
    }

    #[test]
    fn create_menu_offers_project_and_package_only_to_an_org_admin() {
        let admin = layout(
            "t",
            true,
            &signed_in("admin"),
            &PageContext::org("acme"),
            html! {},
        )
        .into_string();
        assert!(admin.contains("New project"));
        assert!(admin.contains("New package"));

        // A reader in the same org gets the menu, but only the one action they
        // can actually complete.
        let reader = layout(
            "t",
            true,
            &signed_in("reader"),
            &PageContext::org("acme"),
            html! {},
        )
        .into_string();
        assert!(!reader.contains("New project"));
        assert!(reader.contains("New organization"));
    }

    #[test]
    fn create_menu_hides_org_scoped_actions_outside_an_org() {
        let markup = layout(
            "t",
            true,
            &signed_in("owner"),
            &PageContext::none(),
            html! {},
        )
        .into_string();
        assert!(!markup.contains("New project"));
        assert!(markup.contains("New organization"));
    }

    #[test]
    fn breadcrumb_reflects_the_route() {
        let markup = layout(
            "t",
            true,
            &signed_in("owner"),
            &PageContext::package("acme", "http"),
            html! {},
        )
        .into_string();
        assert!(markup.contains("/dashboard/acme"));
        assert!(markup.contains("/p/acme/http"));
    }

    #[test]
    fn offline_banner_appears_only_when_the_database_is_down() {
        let offline = layout(
            "t",
            false,
            &Viewer::Anonymous,
            &PageContext::none(),
            html! {},
        )
        .into_string();
        assert!(offline.contains("registry offline"));

        let online = layout(
            "t",
            true,
            &Viewer::Anonymous,
            &PageContext::none(),
            html! {},
        )
        .into_string();
        assert!(!online.contains("registry offline"));
    }

    #[test]
    fn dependency_graph_assets_are_self_hosted() {
        let markup = layout(
            "t",
            true,
            &Viewer::Anonymous,
            &PageContext::none(),
            html! {},
        )
        .into_string();
        assert!(markup.contains("/graph-assets/dependency-graph.css"));
        assert!(markup.contains("/graph-assets/dependency-graph.js"));
        assert!(markup.contains("/static/dependency-graph-insights.css"));
        assert!(markup.contains("/static/dependency-graph-insights.js"));
        assert!(markup.contains("&quot;allowEval&quot;:false"));
        assert!(markup.contains("&quot;allowScriptTags&quot;:false"));
        assert!(!markup.contains("claritas-viz"));
    }
}
