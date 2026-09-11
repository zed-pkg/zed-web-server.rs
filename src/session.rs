//! Resolving the app.zpkg.net product session into a registry viewer.
//!
//! Shared Auth owns the durable session and principal in its dedicated RDS data
//! plane. The web tier keeps only a signed, HttpOnly product cookie containing
//! the opaque rotating refresh handle and canonical principal id. Page reads
//! then resolve that id through the read-only `zed-orm-core` boundary.

use axum::http::HeaderMap;
use uuid::Uuid;
use zed_orm_core::models::{OrgSummary, UserSummary};
use zed_orm_core::{OrmError, ReadContext};

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

/// Recheck one organization membership by its exact composite key.
///
/// `Viewer::orgs` remains the bounded header/switcher projection. It is useful
/// presentation data, but authorization for an addressed private resource must
/// not depend on whether a membership happened to fit inside that page.
pub async fn exact_org_role(
    db: &ReadContext,
    viewer: &Viewer,
    org_id: Uuid,
) -> Result<Option<String>, OrmError> {
    let Some(user) = viewer.user() else {
        return Ok(None);
    };
    zed_orm_core::read::org_role_for_user(db, org_id, user.id).await
}

/// Recheck one direct project membership by its exact composite key.
///
/// Organization and project membership are separate authorities. Callers that
/// accept either scope combine this result with [`exact_org_role`] rather than
/// scanning either page-oriented membership listing.
pub async fn exact_project_role(
    db: &ReadContext,
    viewer: &Viewer,
    project_id: Uuid,
) -> Result<Option<String>, OrmError> {
    let Some(user) = viewer.user() else {
        return Ok(None);
    };
    zed_orm_core::read::project_role_for_user(db, project_id, user.id).await
}

const CUSTOMER_REALM: &str = "customer";

/// Account pages must distinguish an unavailable projection from a new user.
/// Otherwise a failed membership read would offer first-time enrollment to an
/// existing member. These errors deliberately contain no identity or DB data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountResolutionError {
    DatabaseUnavailable,
    ProjectionUnavailable,
}

pub async fn resolve_account(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<Viewer, AccountResolutionError> {
    let Some(subject) = crate::browser_auth::session_subject(state, headers) else {
        return Ok(Viewer::Anonymous);
    };
    let Some(db) = &state.db else {
        return Err(AccountResolutionError::DatabaseUnavailable);
    };

    let user = zed_orm_core::read::user_by_subject(db, CUSTOMER_REALM, subject)
        .await
        .map_err(|_| AccountResolutionError::ProjectionUnavailable)?
        .ok_or(AccountResolutionError::ProjectionUnavailable)?;
    let orgs = zed_orm_core::read::orgs_for_user(db, user.id)
        .await
        .map_err(|_| AccountResolutionError::ProjectionUnavailable)?;

    Ok(Viewer::SignedIn(Box::new(SignedInViewer { user, orgs })))
}

/// Public browsing may fall back to anonymous data. Account entry points use
/// [`resolve_account`] to retain an actionable unavailable state.
pub async fn resolve(state: &WebState, headers: &HeaderMap) -> Viewer {
    resolve_account(state, headers)
        .await
        .unwrap_or(Viewer::Anonymous)
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
