//! Resolving the browser session into a registry viewer.
//!
//! The identity chain is: Supabase Auth (the IdP) → shared-auth-server.rs
//! (owns the principal, issues the session) → `zed_users` (the registry's
//! mirror of that principal). This module walks the last two links.
//!
//! ## Why this calls shared-auth instead of reading the cookie
//!
//! The `__Host-` session cookie is **sealed** with `AUTH_BROWSER_SEAL_SECRET`,
//! which only shared-auth holds. Unsealing it here would mean copying that
//! secret into every product web tier — one leak away from minting sessions for
//! all of them. So the cookie stays opaque and shared-auth is asked.
//!
//! The cookie is already scoped to this origin and reaches shared-auth through
//! the `/shared-auth` reverse proxy in [`crate::proxy`], so forwarding it costs
//! one internal request per page render.
//!
//! ## Fail-closed
//!
//! Every failure — endpoint unset, upstream down, non-200, malformed body,
//! unknown principal — resolves to [`Viewer::Anonymous`]. A signed-in user
//! seeing a signed-out page is a visible annoyance; the reverse is a
//! disclosure, so the ambiguity always resolves toward showing less.

use axum::http::{HeaderMap, header};
use serde::Deserialize;
use zed_orm_core::models::{OrgSummary, UserSummary};

use crate::state::WebState;

/// Who is asking. Every page takes one of these; none of them read the session
/// themselves.
#[derive(Debug, Clone)]
pub enum Viewer {
    Anonymous,
    SignedIn(Box<SignedInViewer>),
}

#[derive(Debug, Clone)]
pub struct SignedInViewer {
    pub user: UserSummary,
    /// Every org the viewer belongs to, resolved once per request because the
    /// header's org switcher needs it on every page anyway.
    pub orgs: Vec<OrgSummary>,
}

impl Viewer {
    #[must_use]
    pub fn user(&self) -> Option<&UserSummary> {
        match self {
            Self::Anonymous => None,
            Self::SignedIn(viewer) => Some(&viewer.user),
        }
    }

    #[must_use]
    pub fn orgs(&self) -> &[OrgSummary] {
        match self {
            Self::Anonymous => &[],
            Self::SignedIn(viewer) => &viewer.orgs,
        }
    }

    #[must_use]
    pub fn is_signed_in(&self) -> bool {
        matches!(self, Self::SignedIn(_))
    }

    /// The viewer's role in an org, if they are a member.
    ///
    /// This is the single gate for showing private rows and management
    /// controls; pages must not infer membership any other way.
    #[must_use]
    pub fn role_in(&self, org_slug: &str) -> Option<&str> {
        self.orgs()
            .iter()
            .find(|org| org.slug == org_slug)
            .map(|org| org.role.as_str())
    }

    /// Whether the viewer may see an org's private projects and packages.
    #[must_use]
    pub fn can_see_private(&self, org_slug: &str) -> bool {
        self.role_in(org_slug).is_some()
    }

    /// Whether the viewer may change org-level settings, invite, or create.
    #[must_use]
    pub fn can_administer(&self, org_slug: &str) -> bool {
        matches!(self.role_in(org_slug), Some("owner" | "admin"))
    }

    /// Org ids the viewer may search across, alongside public packages.
    #[must_use]
    pub fn visible_org_ids(&self) -> Vec<uuid::Uuid> {
        self.orgs().iter().map(|org| org.id).collect()
    }
}

/// The subset of shared-auth's claims this tier needs.
///
/// Deliberately narrow: only enough to identify the principal. Profile fields
/// (email, display name, avatar) are read from the `zed_users` row instead —
/// the API server mirrors them there on sign-in, and having one authoritative
/// source keeps the header from disagreeing with the settings page.
#[derive(Debug, Deserialize)]
struct SessionClaims {
    /// `shared_auth.principals.shared_user_id`.
    sub: uuid::Uuid,
    /// Which auth instance issued this principal. Absent means the customer
    /// realm, which is what a browser session on app.zpkg.net always is.
    #[serde(default)]
    realm: Option<String>,
}

/// Browser sessions on the console are customer-realm by construction; operator
/// tooling authenticates against the admin instance instead.
const DEFAULT_REALM: &str = "customer";

/// Resolve the request's session into a viewer, or [`Viewer::Anonymous`].
pub async fn resolve(state: &WebState, headers: &HeaderMap) -> Viewer {
    let Some(claims) = fetch_claims(state, headers).await else {
        return Viewer::Anonymous;
    };
    let Some(db) = &state.db else {
        // Registry offline: the session is real but nothing can be looked up,
        // so the page renders its offline banner rather than a broken header.
        return Viewer::Anonymous;
    };

    let realm = claims.realm.as_deref().unwrap_or(DEFAULT_REALM);
    let user = match zed_orm_core::read::user_by_subject(db, realm, claims.sub).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            // Authenticated with shared-auth but no registry row yet. The web
            // tier is SELECT-only and cannot create one; the API server does
            // that on first console request. Until then, anonymous.
            tracing::debug!(subject = %claims.sub, "no registry user for principal yet");
            return Viewer::Anonymous;
        }
        Err(error) => {
            tracing::warn!(%error, "registry user lookup failed");
            return Viewer::Anonymous;
        }
    };

    let orgs = zed_orm_core::read::orgs_for_user(db, user.id)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "org membership lookup failed");
            Vec::new()
        });

    Viewer::SignedIn(Box::new(SignedInViewer { user, orgs }))
}

/// Ask shared-auth to turn the sealed cookie into claims.
async fn fetch_claims(state: &WebState, headers: &HeaderMap) -> Option<SessionClaims> {
    let base = state.shared_auth_url.as_deref()?;
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    // No cookie, no session — skip the upstream call entirely rather than
    // spending a request to be told the obvious.
    if cookie.is_empty() {
        return None;
    }

    let endpoint = format!("{base}{}", state.session_path);
    let response = state
        .http
        .get(&endpoint)
        .header(header::COOKIE, cookie)
        .send()
        .await
        .inspect_err(|error| tracing::debug!(%error, "shared-auth session lookup failed"))
        .ok()?;

    if !response.status().is_success() {
        return None;
    }
    response
        .json::<SessionClaims>()
        .await
        .inspect_err(|error| tracing::warn!(%error, "shared-auth session body was not claims"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zed_orm_core::models::OrgSummary;

    fn viewer_with(role: &str, slug: &str) -> Viewer {
        Viewer::SignedIn(Box::new(SignedInViewer {
            user: UserSummary {
                id: uuid::Uuid::nil(),
                subject: uuid::Uuid::nil(),
                realm: "customer".into(),
                email: None,
                display_name: None,
                avatar_url: None,
                settings: serde_json::Value::Object(Default::default()),
            },
            orgs: vec![OrgSummary {
                id: uuid::Uuid::nil(),
                slug: slug.into(),
                name: slug.into(),
                description: None,
                role: role.into(),
            }],
        }))
    }

    #[test]
    fn anonymous_sees_nothing_private_and_administers_nothing() {
        let viewer = Viewer::Anonymous;
        assert!(!viewer.is_signed_in());
        assert!(!viewer.can_see_private("acme"));
        assert!(!viewer.can_administer("acme"));
        assert!(viewer.visible_org_ids().is_empty());
    }

    #[test]
    fn membership_grants_private_visibility_in_that_org_only() {
        let viewer = viewer_with("member", "acme");
        assert!(viewer.can_see_private("acme"));
        assert!(!viewer.can_see_private("other"));
    }

    #[test]
    fn only_owners_and_admins_administer() {
        assert!(viewer_with("owner", "acme").can_administer("acme"));
        assert!(viewer_with("admin", "acme").can_administer("acme"));
        // A member can see private packages but must not get management
        // controls — the two checks are deliberately separate.
        assert!(!viewer_with("member", "acme").can_administer("acme"));
        assert!(!viewer_with("reader", "acme").can_administer("acme"));
    }

    #[test]
    fn a_member_of_one_org_cannot_administer_another() {
        let viewer = viewer_with("owner", "acme");
        assert!(!viewer.can_administer("globex"));
    }
}
