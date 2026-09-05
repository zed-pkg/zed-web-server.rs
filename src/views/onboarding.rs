//! Pure, escaped presentation for the two customer account journeys.
//! The same customer identity can own personal namespaces and join teams.

use maud::{Markup, html};

use crate::session::SignedInViewer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Journey {
    Individual,
    Organization,
}

impl Journey {
    pub const fn path(self) -> &'static str {
        match self {
            Self::Individual => "/onboarding/individual",
            Self::Organization => "/onboarding/organization",
        }
    }

    pub const fn sign_in_path(self) -> &'static str {
        match self {
            Self::Individual => "/auth/sign-in?return_to=%2Fonboarding%2Findividual",
            Self::Organization => "/auth/sign-in?return_to=%2Fonboarding%2Forganization",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Individual => "Your personal workspace",
            Self::Organization => "Your organization workspace",
        }
    }
}

pub enum Stage<'a> {
    SignIn,
    Active(&'a SignedInViewer),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentError {
    Invalid,
    Conflict,
    Denied,
    Unavailable,
}

impl EnrollmentError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Invalid => "Check the namespace and display name, then try again.",
            Self::Conflict => {
                "That namespace could not be created. Choose another name or check your existing workspaces."
            }
            Self::Denied => "Your account cannot create a namespace right now.",
            Self::Unavailable => {
                "We could not confirm that the workspace was created. Check your workspaces before trying again."
            }
        }
    }
}

#[derive(Default)]
pub struct FormValues<'a> {
    pub slug: &'a str,
    pub name: &'a str,
}

pub fn choose() -> Markup {
    html! {
        section class="onboarding-hero" aria-labelledby="onboarding-title" {
            p class="eyebrow" { "Start with Zed" }
            h1 id="onboarding-title" { "A workspace that fits the way you build." }
            p class="lede" {
                "Manage your own packages or collaborate with your team. "
                "One account can do both."
            }
        }
        div class="journey-grid" {
            article class="card journey-card" aria-labelledby="individual-title" {
                span class="badge" { "Individual" }
                h2 id="individual-title" { "Build on your own" }
                p { "Find packages, create a publishing namespace, and manage your profile." }
                ul class="journey-features" {
                    li { "Use Zed for a single project or ongoing work." }
                    li { "Keep your personal workflow separate from team workspaces." }
                }
                a class="button primary" href=(Journey::Individual.path()) { "Continue as an individual" }
            }
            article class="card journey-card" aria-labelledby="organization-title" {
                span class="badge" { "Organization" }
                h2 id="organization-title" { "Bring your team together" }
                p { "Create an organization or open a workspace you already belong to." }
                ul class="journey-features" {
                    li { "Group packages into projects." }
                    li { "Invite colleagues with their own accounts and explicit roles." }
                }
                a class="button primary" href=(Journey::Organization.path()) { "Continue with an organization" }
            }
        }
        p class="muted" { "Just looking? " a href="/search" { "Browse public packages without signing in." } }
    }
}

pub fn page(
    journey: Journey,
    stage: Stage<'_>,
    failure: Option<EnrollmentError>,
    values: FormValues<'_>,
) -> Markup {
    html! {
        a class="onboarding-back" href="/onboarding" { "← Choose how you use Zed" }
        section class="onboarding-hero" {
            p class="eyebrow" {
                @match journey {
                    Journey::Individual => "Individual account",
                    Journey::Organization => "Organization account",
                }
            }
            h1 { (journey.title()) }
            p class="lede" {
                @match journey {
                    Journey::Individual => "Your account is yours. Start with a package search or create a namespace when you are ready to publish.",
                    Journey::Organization => "Each colleague signs in with their own account. Organization membership controls access to your team's projects and packages.",
                }
            }
        }
        @if let Some(failure) = failure {
            div class="onboarding-notice" role="alert" { (failure.message()) }
        }
        @match stage {
            Stage::SignIn => {
                section class="card" aria-labelledby="signin-title" {
                    h2 id="signin-title" { "First, sign in or create your account" }
                    p { "Continue through our secure sign-in service. You will return here to finish setting up your workspace." }
                    a class="button primary" href=(journey.sign_in_path()) { "Sign in or create an account" }
                }
                @if journey == Journey::Organization {
                    p class="muted" { "Joining an existing team? Use your invitation and the account it was sent to. A company email address alone does not grant membership." }
                }
            },
            Stage::Active(viewer) => {
                div class="onboarding-status" { span class="badge" { "Signed in" } " Choose your next step." }
                @if journey == Journey::Individual {
                    div class="row" {
                        a class="button" href="/search" { "Find a package" }
                        a class="button" href="/settings" { "Manage your profile" }
                    }
                }
                @if !viewer.orgs.is_empty() {
                    section class="card" aria-labelledby="workspaces-title" {
                        h2 id="workspaces-title" { "Your workspaces" }
                        p class="muted" { "Choose where you want to work. Your role is shown for each workspace." }
                        ul class="org-list" {
                            @for org in &viewer.orgs {
                                li {
                                    a class="org-name" href={ "/dashboard/" (org.slug) } { (org.name) }
                                    span class="badge" { (org.role) }
                                    @if matches!(org.role.as_str(), "owner" | "admin") {
                                        a href={ "/orgs/" (org.slug) "/settings" } { "Manage team and projects" }
                                    }
                                }
                            }
                        }
                    }
                }
                (enrollment_form(journey, values))
                @if journey == Journey::Organization {
                    section class="card" aria-labelledby="next-title" {
                        h2 id="next-title" { "After creating your organization" }
                        ol class="journey-features" {
                            li { "Open your workspace and create a project." }
                            li { "Invite each colleague from organization settings." }
                            li { "Choose reader, member, or admin access for each invitation." }
                        }
                        p class="muted" { "Organization admins manage their own team. Platform administration uses a separate, private application." }
                    }
                }
            },
            Stage::Unavailable => {
                section class="card" aria-labelledby="unavailable-title" {
                    h2 id="unavailable-title" { "Account setup is temporarily unavailable" }
                    p { "We cannot verify your account and workspaces right now. Your existing account has not been changed." }
                    a class="button primary" href=(journey.path()) { "Try again" }
                    a class="button" href="/search" { "Browse public packages" }
                }
            },
        }
    }
}

fn enrollment_form(journey: Journey, values: FormValues<'_>) -> Markup {
    html! {
        section class="card" aria-labelledby="create-title" {
            h2 id="create-title" {
                @match journey {
                    Journey::Individual => "Create a publishing namespace",
                    Journey::Organization => "Create an organization",
                }
            }
            p class="muted" {
                "A namespace is the first part of a package address, such as acme/http-client. "
                "You become its owner."
            }
            form class="stack" method="post" action=(journey.path()) {
                label for="onboarding-slug" { "Namespace" }
                input id="onboarding-slug" name="slug" required minlength="2" maxlength="64"
                    pattern="[a-z0-9][a-z0-9-]{0,62}[a-z0-9]" autocomplete="off"
                    aria-describedby="namespace-help" placeholder="your-namespace" value=(values.slug);
                p id="namespace-help" class="field-help" { "Use 2–64 lowercase letters, numbers, and internal hyphens." }
                label for="onboarding-name" { "Display name" }
                input id="onboarding-name" name="name" required maxlength="200"
                    autocomplete="organization" placeholder="Your name or team" value=(values.name);
                button class="button primary" type="submit" {
                    @match journey {
                        Journey::Individual => "Create namespace",
                        Journey::Organization => "Create organization",
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zed_orm_core::models::{OrgSummary, UserSummary};

    fn viewer(role: &str) -> SignedInViewer {
        SignedInViewer {
            user: UserSummary {
                id: uuid::Uuid::nil(),
                subject: uuid::Uuid::nil(),
                realm: "customer".into(),
                email: None,
                display_name: None,
                avatar_url: None,
                settings: serde_json::json!({}),
            },
            orgs: vec![OrgSummary {
                id: uuid::Uuid::nil(),
                slug: "acme".into(),
                name: "Acme <script>alert(1)</script>".into(),
                description: None,
                role: role.into(),
            }],
        }
    }

    #[test]
    fn every_journey_has_a_distinct_local_sign_in_return() {
        for journey in [Journey::Individual, Journey::Organization] {
            let markup = page(journey, Stage::SignIn, None, FormValues::default()).into_string();
            assert!(markup.contains(journey.sign_in_path()));
            assert!(!markup.contains("<form"));
        }
        assert_ne!(
            Journey::Individual.sign_in_path(),
            Journey::Organization.sign_in_path()
        );
    }

    #[test]
    fn only_active_accounts_receive_same_origin_creation_forms() {
        let account = viewer("member");
        for journey in [Journey::Individual, Journey::Organization] {
            let active = page(
                journey,
                Stage::Active(&account),
                None,
                FormValues::default(),
            )
            .into_string();
            assert!(active.contains(&format!("method=\"post\" action=\"{}\"", journey.path())));
            let unavailable =
                page(journey, Stage::Unavailable, None, FormValues::default()).into_string();
            assert!(!unavailable.contains("<form"));
            assert!(!unavailable.contains("/auth/sign-in"));
        }
    }

    #[test]
    fn workspaces_are_explicit_and_management_links_follow_membership_roles() {
        for role in ["owner", "admin", "member", "reader"] {
            let account = viewer(role);
            let markup = page(
                Journey::Organization,
                Stage::Active(&account),
                None,
                FormValues::default(),
            )
            .into_string();
            assert!(markup.contains("/dashboard/acme"));
            assert_eq!(
                markup.contains("/orgs/acme/settings"),
                matches!(role, "owner" | "admin")
            );
            assert!(!markup.contains("<script>"));
            assert!(markup.contains("&lt;script&gt;"));
        }
    }

    #[test]
    fn failed_forms_preserve_escaped_values_without_an_upstream_error_body() {
        let account = viewer("owner");
        let markup = page(
            Journey::Organization,
            Stage::Active(&account),
            Some(EnrollmentError::Conflict),
            FormValues {
                slug: "existing",
                name: "\"><script>alert(1)</script>",
            },
        )
        .into_string();
        assert!(markup.contains("role=\"alert\""));
        assert!(markup.contains("value=\"existing\""));
        assert!(!markup.contains("<script>"));
        assert!(markup.contains("&lt;script&gt;"));
    }
}
