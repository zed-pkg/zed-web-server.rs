//! Resolving the app.zpkg.net product session into a registry viewer.
//!
//! Shared Auth owns the durable session and principal in its dedicated RDS data
//! plane. The web tier keeps only a signed, HttpOnly product cookie containing
//! the opaque rotating refresh handle and canonical principal id. Page reads
//! then resolve that id through the read-only `zed-orm-core` boundary.

use axum::http::HeaderMap;
use zed_orm_core::models::{OrgSummary, UserSummary};

use crate::state::WebState;

/// Who is asking. Every page takes one of these; none of them interpret cookies
/// or query Shared Auth independently.
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

    #[must_use]
    pub fn role_in(&self, org_slug: &str) -> Option<&str> {
        self.orgs()
            .iter()
            .find(|org| org.slug == org_slug)
            .map(|org| org.role.as_str())
    }

    #[must_use]
    pub fn can_see_private(&self, org_slug: &str) -> bool {
        self.role_in(org_slug).is_some()
    }

    #[must_use]
    pub fn can_administer(&self, org_slug: &str) -> bool {
        matches!(self.role_in(org_slug), Some("owner" | "admin"))
    }

    #[must_use]
    pub fn visible_org_ids(&self) -> Vec<uuid::Uuid> {
        self.orgs().iter().map(|org| org.id).collect()
    }
}

const CUSTOMER_REALM: &str = "customer";

/// Resolve a valid product session into the local registry projection.
///
/// Every failure resolves to anonymous. A signed-in user seeing a signed-out
/// page is visible; accidentally disclosing a private row is not.
pub async fn resolve(state: &WebState, headers: &HeaderMap) -> Viewer {
    let Some(subject) = crate::browser_auth::session_subject(state, headers) else {
        return Viewer::Anonymous;
    };
    let Some(db) = &state.db else {
        return Viewer::Anonymous;
    };

    let user = match zed_orm_core::read::user_by_subject(db, CUSTOMER_REALM, subject).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            tracing::debug!(%subject, "no registry user projection for Shared Auth principal");
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

#[cfg(test)]
mod tests {
    use super::*;
    use zed_orm_core::models::OrgSummary;

    fn viewer_with(role: &str, slug: &str) -> Viewer {
        Viewer::SignedIn(Box::new(SignedInViewer {
            user: UserSummary {
                id: uuid::Uuid::nil(),
                subject: uuid::Uuid::nil(),
                realm: CUSTOMER_REALM.into(),
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
        assert!(!viewer_with("member", "acme").can_administer("acme"));
        assert!(!viewer_with("reader", "acme").can_administer("acme"));
    }

    #[test]
    fn a_member_of_one_org_cannot_administer_another() {
        let viewer = viewer_with("owner", "acme");
        assert!(!viewer.can_administer("globex"));
    }
}
