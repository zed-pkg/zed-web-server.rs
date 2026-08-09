use std::sync::Arc;

use axum::extract::State;

use crate::state::WebState;

/// Liveness plus a cheap database probe. Never fails the process: the site
/// serves in offline mode without Postgres, so this reports rather than gates.
pub async fn healthz(State(state): State<Arc<WebState>>) -> axum::Json<serde_json::Value> {
    let db = match &state.db {
        Some(context) => zed_orm_core::read::ping(context).await.is_ok(),
        None => false,
    };
    axum::Json(serde_json::json!({
        "ok": true,
        "db": db,
        "registry": state.registry_url,
        "shared_auth": state.shared_auth_url.is_some(),
    }))
}
