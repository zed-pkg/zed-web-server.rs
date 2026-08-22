//! Bounded Maud fragments for the dependency-graph component.
//!
//! The SVG model remains browser-local because it composes independently
//! authorized immutable graph documents. HTMX posts only presentation data
//! from that already-visible model; these endpoints never perform a package
//! read or make an authorization decision. Every field is bounded and Maud
//! escapes it before returning same-origin HTML. A rejected fragment leaves the
//! component's local semantic renderer in place.

use axum::Form;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use maud::{Markup, html};
use serde::Deserialize;

const QUERY_PAGE_SIZE: usize = 25;
const MAX_QUERY_RESULTS: usize = 3_000;
const MAX_RELATIONSHIPS: usize = 12_000;
const MAX_FRAGMENT_RELATIONSHIPS: usize = 250;
const MAX_NEIGHBORS: usize = 18;
const MAX_LIST_ITEMS: usize = 256;
const MAX_LABEL: usize = 256;
const MAX_IDENTITY: usize = 2_048;
const MAX_METADATA: usize = 512;
const MAX_LOCAL_URL: usize = 4_096;

#[derive(Deserialize)]
pub struct QueryFragmentForm {
    label: String,
    title_id: String,
    page: usize,
    total: usize,
    rows: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryRow {
    id: String,
    org: String,
    name: String,
    version: String,
    license: String,
    updated_at: String,
    dependencies: usize,
    dependents: usize,
}

pub async fn query(Form(form): Form<QueryFragmentForm>) -> Response {
    let Ok(rows) = serde_json::from_str::<Vec<QueryRow>>(&form.rows) else {
        return invalid_fragment();
    };
    if !valid_query(&form, &rows) {
        return invalid_fragment();
    }
    Html(render_query(&form, &rows).into_string()).into_response()
}

fn valid_query(form: &QueryFragmentForm, rows: &[QueryRow]) -> bool {
    if !valid_text(&form.label, MAX_LABEL, false)
        || !valid_dom_id(&form.title_id)
        || form.total > MAX_QUERY_RESULTS
        || rows.len() > QUERY_PAGE_SIZE
    {
        return false;
    }
    let page_count = form.total.div_ceil(QUERY_PAGE_SIZE).max(1);
    if form.page >= page_count {
        return false;
    }
    let expected_rows = form
        .total
        .saturating_sub(form.page * QUERY_PAGE_SIZE)
        .min(QUERY_PAGE_SIZE);
    rows.len() == expected_rows
        && rows.iter().all(|row| {
            valid_text(&row.id, MAX_IDENTITY, false)
                && valid_text(&row.org, MAX_METADATA, false)
                && valid_text(&row.name, MAX_METADATA, false)
                && valid_text(&row.version, MAX_METADATA, true)
                && valid_text(&row.license, MAX_METADATA, true)
                && valid_text(&row.updated_at, 64, true)
                && row.dependencies <= MAX_RELATIONSHIPS
                && row.dependents <= MAX_RELATIONSHIPS
        })
}

fn render_query(form: &QueryFragmentForm, rows: &[QueryRow]) -> Markup {
    let page_count = form.total.div_ceil(QUERY_PAGE_SIZE).max(1);
    let first = if form.total == 0 {
        0
    } else {
        form.page * QUERY_PAGE_SIZE + 1
    };
    let last = (form.page * QUERY_PAGE_SIZE + rows.len()).min(form.total);
    html! {
        div data-fragment-source="htmx" {
            div class="dg-query-summary-head" {
                div {
                    p class="dg-eyebrow" { "Accessible query result" }
                    h3 id=(form.title_id) { (form.label) }
                    p {
                        @if form.total == 0 {
                            "No packages matched this analysis."
                        } @else {
                            "Showing " (first) "–" (last) " of " (form.total) " packages."
                        }
                    }
                }
                @if page_count > 1 {
                    nav aria-label="Query result pages" {
                        button type="button" data-query-page="previous" disabled[form.page == 0] {
                            "Previous"
                        }
                        span { "Page " (form.page + 1) " of " (page_count) }
                        button type="button" data-query-page="next" disabled[form.page + 1 == page_count] {
                            "Next"
                        }
                    }
                }
            }
            @if !rows.is_empty() {
                div class="dg-query-table" {
                    table {
                        caption { (form.label) }
                        thead {
                            tr {
                                th scope="col" { "Package" }
                                th scope="col" { "Version" }
                                th scope="col" { "License" }
                                th scope="col" { "Updated" }
                                th scope="col" { "Dependencies" }
                                th scope="col" { "Dependents" }
                                th scope="col" { "Action" }
                            }
                        }
                        tbody {
                            @for row in rows {
                                tr {
                                    th scope="row" {
                                        a href=(package_url(&row.org, &row.name)) {
                                            (row.org) "/" (row.name)
                                        }
                                    }
                                    td { (display_or(&row.version, "unresolved")) }
                                    td { (display_or(&row.license, "—")) }
                                    td { (display_or(&row.updated_at, "—")) }
                                    td { (row.dependencies) }
                                    td { (row.dependents) }
                                    td {
                                        button type="button" data-query-select=(row.id) { "Inspect" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize)]
pub struct InspectorFragmentForm {
    node: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectorNode {
    org: String,
    name: String,
    registry_id: String,
    version: String,
    requirements: Vec<String>,
    dependencies: usize,
    dependents: usize,
    features: Vec<String>,
    license: String,
    updated_at: String,
    artifact: String,
    synthetic: bool,
    expandable: bool,
    outgoing: Vec<InspectorNeighbor>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectorNeighbor {
    id: String,
    org: String,
    name: String,
    kind: String,
    requirement: String,
}

pub async fn inspector(Form(form): Form<InspectorFragmentForm>) -> Response {
    let Ok(node) = serde_json::from_str::<InspectorNode>(&form.node) else {
        return invalid_fragment();
    };
    if !valid_inspector(&node) {
        return invalid_fragment();
    }
    Html(render_inspector(&node).into_string()).into_response()
}

fn valid_inspector(node: &InspectorNode) -> bool {
    valid_text(&node.org, MAX_METADATA, false)
        && valid_text(&node.name, MAX_METADATA, false)
        && valid_text(&node.registry_id, MAX_IDENTITY, false)
        && valid_text(&node.version, MAX_METADATA, true)
        && valid_list(&node.requirements)
        && valid_list(&node.features)
        && node.dependencies <= MAX_RELATIONSHIPS
        && node.dependents <= MAX_RELATIONSHIPS
        && valid_text(&node.license, MAX_METADATA, true)
        && valid_text(&node.updated_at, 64, true)
        && valid_text(&node.artifact, MAX_METADATA, true)
        && node.outgoing.len() <= MAX_NEIGHBORS
        && node.outgoing.iter().all(|neighbor| {
            valid_text(&neighbor.id, MAX_IDENTITY, false)
                && valid_text(&neighbor.org, MAX_METADATA, false)
                && valid_text(&neighbor.name, MAX_METADATA, false)
                && valid_text(&neighbor.kind, 64, false)
                && valid_text(&neighbor.requirement, MAX_METADATA, true)
        })
}

fn render_inspector(node: &InspectorNode) -> Markup {
    html! {
        div data-fragment-source="htmx" {
            div class="dg-inspector-head" {
                p class="dg-eyebrow" { "Selected package" }
                h3 { (node.org) "/" (node.name) }
                p class="dg-identity" { (node.registry_id) }
            }
            dl class="dg-detail-grid" {
                dt { "Version" } dd { (display_or(&node.version, "not resolved")) }
                dt { "Requirements" } dd { (list_or_dash(&node.requirements)) }
                dt { "Dependencies" } dd { (node.dependencies) }
                dt { "Dependents" } dd { (node.dependents) }
                dt { "Features" } dd { (list_or_dash(&node.features)) }
                dt { "License" } dd { (display_or(&node.license, "—")) }
                dt { "Last metadata update" } dd { (display_or(&node.updated_at, "—")) }
                dt { "Artifact" } dd class="dg-digest" { (display_or(&node.artifact, "—")) }
            }
            div class="dg-inspector-actions" {
                a class="button" href=(package_url(&node.org, &node.name)) { "Open package" }
                @if node.expandable {
                    button class="button primary" type="button" data-inspector-action="expand" {
                        "Expand latest declared graph"
                    }
                }
            }
            @if node.synthetic {
                p class="dg-caveat" {
                    "This node was expanded using its latest declared manifest. It is navigation context, not an exact lockfile resolution."
                }
            }
            div class="dg-neighbor-list" {
                h4 { "Outgoing relationships" }
                @if node.outgoing.is_empty() {
                    p { "No outgoing relationships in the loaded graph." }
                } @else {
                    @for neighbor in &node.outgoing {
                        button type="button" data-select-node=(neighbor.id) {
                            span { (neighbor.org) "/" (neighbor.name) }
                            small {
                                (neighbor.kind)
                                @if !neighbor.requirement.is_empty() {
                                    " · " (neighbor.requirement)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize)]
pub struct TableFragmentForm {
    total: usize,
    rows: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationshipRow {
    from_org: String,
    from_name: String,
    to_org: String,
    to_name: String,
    kind: String,
    requirement: String,
    optional: bool,
}

pub async fn table(Form(form): Form<TableFragmentForm>) -> Response {
    let Ok(rows) = serde_json::from_str::<Vec<RelationshipRow>>(&form.rows) else {
        return invalid_fragment();
    };
    if !valid_table(&form, &rows) {
        return invalid_fragment();
    }
    Html(render_table(form.total, &rows).into_string()).into_response()
}

fn valid_table(form: &TableFragmentForm, rows: &[RelationshipRow]) -> bool {
    form.total <= MAX_RELATIONSHIPS
        && rows.len() <= MAX_FRAGMENT_RELATIONSHIPS
        && rows.len() <= form.total
        && rows.iter().all(|row| {
            valid_text(&row.from_org, MAX_METADATA, false)
                && valid_text(&row.from_name, MAX_METADATA, false)
                && valid_text(&row.to_org, MAX_METADATA, false)
                && valid_text(&row.to_name, MAX_METADATA, false)
                && valid_text(&row.kind, 64, false)
                && valid_text(&row.requirement, MAX_METADATA, true)
        })
}

fn render_table(total: usize, rows: &[RelationshipRow]) -> Markup {
    html! {
        div data-fragment-source="htmx" {
            @if rows.len() < total {
                p role="status" {
                    "Showing the first " (rows.len()) " of " (total) " relationships in the server-rendered table to keep this fragment bounded. Use the canonical CSV exports for complete semantic edge data."
                }
            }
            @if rows.is_empty() {
                p { "No dependency relationships are loaded." }
            } @else {
                table {
                    caption { "Loaded dependency relationships" }
                    thead {
                        tr {
                            th scope="col" { "From" }
                            th scope="col" { "To" }
                            th scope="col" { "Kind" }
                            th scope="col" { "Requirement" }
                            th scope="col" { "Optional" }
                        }
                    }
                    tbody {
                        @for row in rows {
                            tr {
                                td {
                                    a href=(package_url(&row.from_org, &row.from_name)) {
                                        (row.from_org) "/" (row.from_name)
                                    }
                                }
                                td {
                                    a href=(package_url(&row.to_org, &row.to_name)) {
                                        (row.to_org) "/" (row.to_name)
                                    }
                                }
                                td { (row.kind) }
                                td { (display_or(&row.requirement, "—")) }
                                td { @if row.optional { "yes" } @else { "no" } }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize)]
pub struct StateFragmentForm {
    action: String,
    url: String,
}

pub async fn state(Form(form): Form<StateFragmentForm>) -> Response {
    let message = match form.action.as_str() {
        "save" => "Saved view state is mirrored in this reproducible URL.",
        "restore" => "Restored view state is mirrored in this reproducible URL.",
        "share" => "This URL reproduces the current dependency graph view.",
        _ => return invalid_fragment(),
    };
    if !valid_local_url(&form.url) {
        return invalid_fragment();
    }
    Html(
        html! {
            p data-fragment-source="htmx" role="status" {
                (message) " " a href=(form.url) { "Open reproducible view" }
            }
        }
        .into_string(),
    )
    .into_response()
}

fn valid_list(values: &[String]) -> bool {
    values.len() <= MAX_LIST_ITEMS
        && values
            .iter()
            .all(|value| valid_text(value, MAX_METADATA, false))
}

fn valid_text(value: &str, max: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty()) && value.len() <= max && !value.chars().any(char::is_control)
}

fn valid_local_url(value: &str) -> bool {
    value.starts_with('/')
        && !value.starts_with("//")
        && value.len() <= MAX_LOCAL_URL
        && !value.chars().any(char::is_control)
}

fn valid_dom_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

fn display_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() { fallback } else { value }
}

fn list_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "—".into()
    } else {
        values.join(", ")
    }
}

fn package_url(org: &str, name: &str) -> String {
    format!("/p/{}/{}", uri_segment(org), uri_segment(name))
}

fn uri_segment(value: &str) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    encoded
}

fn invalid_fragment() -> Response {
    (StatusCode::BAD_REQUEST, "invalid dependency graph fragment").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_row(id: &str) -> QueryRow {
        QueryRow {
            id: id.into(),
            org: "acme".into(),
            name: "http".into(),
            version: "1.0.0".into(),
            license: "Apache-2.0".into(),
            updated_at: "2026-08-22".into(),
            dependencies: 2,
            dependents: 3,
        }
    }

    #[test]
    fn query_fragment_is_paginated_and_escapes_presentation_data() {
        let form = QueryFragmentForm {
            label: "<Aggregate & review>".into(),
            title_id: "dg-test-query-title".into(),
            page: 0,
            total: 26,
            rows: String::new(),
        };
        let rows = (0..25)
            .map(|index| query_row(&format!("node-{index}")))
            .collect::<Vec<_>>();
        assert!(valid_query(&form, &rows));
        let markup = render_query(&form, &rows).into_string();
        assert!(markup.contains("&lt;Aggregate &amp; review&gt;"));
        assert!(markup.contains("Page 1 of 2"));
        assert!(markup.contains("data-query-page=\"next\""));
        assert!(markup.contains("data-fragment-source=\"htmx\""));
    }

    #[test]
    fn fragment_bounds_reject_inconsistent_or_oversized_models() {
        let form = QueryFragmentForm {
            label: "Topology".into(),
            title_id: "dg-test-query-title".into(),
            page: 1,
            total: 1,
            rows: String::new(),
        };
        assert!(!valid_query(&form, &[]));
        assert!(!valid_local_url("https://attacker.invalid/graph"));
        assert!(!valid_local_url("//attacker.invalid/graph"));
        assert!(valid_local_url(
            "/dashboard/acme/dependency-graph?graph-query=cycles#dependency-graph"
        ));
    }

    #[test]
    fn relationship_fragment_encodes_package_paths() {
        let rows = vec![RelationshipRow {
            from_org: "acme tools".into(),
            from_name: "http/client".into(),
            to_org: "vendor".into(),
            to_name: "tls".into(),
            kind: "runtime".into(),
            requirement: "^1".into(),
            optional: false,
        }];
        let markup = render_table(1, &rows).into_string();
        assert!(markup.contains("/p/acme%20tools/http%2Fclient"));
        assert!(markup.contains("Loaded dependency relationships"));
    }
}
