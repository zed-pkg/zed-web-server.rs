//! `/search` and its HTMX fragment.
//!
//! Search always unions public packages with the orgs the viewer belongs to.
//! The org list comes from the resolved session, never from a query parameter,
//! so a caller cannot widen their own scope.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Html;
use maud::html;
use serde::Deserialize;

use crate::session;
use crate::state::WebState;
use crate::views::{PageContext, components, layout};

const SEARCH_LIMIT: u64 = 50;

#[derive(Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
}

async fn results(
    state: &WebState,
    viewer: &session::Viewer,
    query: &str,
) -> Vec<components::PackageRow> {
    let Some(db) = &state.db else {
        return Vec::new();
    };
    zed_orm_core::read::search_packages(db, query, &viewer.visible_org_ids(), SEARCH_LIMIT)
        .await
        .unwrap_or_default()
        .iter()
        .map(super::package::summary_row)
        .collect()
}

pub async fn page(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Html<String> {
    let viewer = session::resolve(&state, &headers).await;
    let rows = results(&state, &viewer, &params.q).await;

    let content = html! {
        h1 { "Search" }
        (components::search_box(&params.q))
        @if !params.q.trim().is_empty() {
            div id="results" { (components::package_rows(&rows, "No packages matched.")) }
        }
    };

    Html(
        layout(
            "Search",
            state.db.is_some(),
            &viewer,
            &PageContext::none(),
            content,
        )
        .into_string(),
    )
}

/// HTMX fragment: the results list alone, never a full layout.
pub async fn partial(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Html<String> {
    let viewer = session::resolve(&state, &headers).await;
    let rows = results(&state, &viewer, &params.q).await;
    Html(components::package_rows(&rows, "No packages matched.").into_string())
}
