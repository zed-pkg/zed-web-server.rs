//! Reusable page fragments: search box, package lists, version tables.

use maud::{Markup, html};

pub fn search_box(query: &str) -> Markup {
    html! {
        input
            id="q"
            class="search"
            type="search"
            name="q"
            value=(query)
            placeholder="search packages, e.g. http"
            hx-get="/partials/search"
            hx-trigger="input changed delay:300ms, search"
            hx-target="#results"
            autofocus;
        div id="results" {}
    }
}

pub struct PackageRow {
    pub org: String,
    pub name: String,
    pub description: Option<String>,
    pub latest: Option<String>,
    /// Exact latest-version channel metadata for dependency topology filters.
    pub latest_prerelease: bool,
    pub visibility: String,
}

pub fn package_rows(rows: &[PackageRow], empty_message: &str) -> Markup {
    html! {
        @if rows.is_empty() {
            p class="muted" { (empty_message) }
        } @else {
            ul class="pkg-list" {
                @for row in rows {
                    li {
                        a class="pkg-name" href={ "/p/" (row.org) "/" (row.name) } {
                            (row.org) "/" (row.name)
                        }
                        @if let Some(latest) = &row.latest {
                            span class="pkg-version" { "v" (latest) }
                        }
                        @if let Some(description) = &row.description {
                            span class="pkg-desc" { (description) }
                        }
                    }
                }
            }
        }
    }
}

pub struct VersionRow {
    pub version: String,
    pub published_at: String,
    pub size: i64,
    pub sha256: String,
    pub vcs_tag: String,
    pub yanked: bool,
}

pub fn install_snippet(org: &str, name: &str) -> Markup {
    html! {
        pre class="snippet" { "zed add " (org) "/" (name) }
    }
}

pub fn version_table(rows: &[VersionRow]) -> Markup {
    html! {
        table class="versions" {
            thead {
                tr {
                    th { "version" }
                    th { "published" }
                    th { "size" }
                    th { "sha256" }
                    th { "provenance" }
                }
            }
            tbody {
                @for row in rows {
                    tr class=@if row.yanked { "yanked" } @else { "" } {
                        td class="mono" { (row.version) @if row.yanked { " (yanked)" } }
                        td { (row.published_at) }
                        td { (human_size(row.size)) }
                        td class="mono blue" { (short_sha(&row.sha256)) }
                        td class="mono blue" { "tag " (row.vcs_tag) }
                    }
                }
            }
        }
    }
}

pub fn human_size(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    if bytes < 1024.0 {
        return format!("{bytes:.0} B");
    }
    let kib = bytes / 1024.0;
    if kib < 1024.0 {
        return format!("{kib:.1} KiB");
    }
    let mib = kib / 1024.0;
    if mib < 1024.0 {
        return format!("{mib:.1} MiB");
    }
    format!("{:.1} GiB", mib / 1024.0)
}

pub fn short_sha(sha256: &str) -> String {
    sha256.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_box_is_wired_for_htmx() {
        let markup = search_box("http").into_string();
        assert!(markup.contains(r#"hx-get="/partials/search""#));
        assert!(markup.contains(r##"hx-target="#results""##));
        assert!(markup.contains(r#"value="http""#));
    }

    #[test]
    fn sizes_humanize() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(4096), "4.0 KiB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn version_table_shows_provenance() {
        let rows = vec![VersionRow {
            version: "1.2.0".into(),
            published_at: "2026-07-23".into(),
            size: 4096,
            sha256: "9f3ac2deadbeef00".into(),
            vcs_tag: "v1.2.0".into(),
            yanked: false,
        }];
        let markup = version_table(&rows).into_string();
        assert!(markup.contains("tag v1.2.0"));
        assert!(markup.contains("9f3ac2deadbe"));
    }
}
