//! Shared Maud components for package, project, and organization graph views.

use std::fmt::Write as _;

use maud::{Markup, html};
use serde_json::json;

use super::components::PackageRow;

#[derive(Debug, Clone)]
pub struct GraphVersion {
    pub version: String,
    pub prerelease: bool,
    pub yanked: bool,
}

const FALLBACK_EXPORTS: &[(&str, &str)] = &[
    ("json", "JSON"),
    ("yaml", "YAML"),
    ("toml", "TOML"),
    ("json5", "JSON5"),
    ("xml", "XML"),
    ("csv", "CSV"),
    ("msgpack", "MessagePack"),
    ("protobuf", "Protocol Buffers"),
    ("dot", "Graphviz DOT"),
    ("mermaid", "Mermaid"),
];

pub fn package_workspace(
    org: &str,
    name: &str,
    selected_version: &str,
    versions: &[GraphVersion],
    is_private: bool,
) -> Markup {
    let versions_json = serde_json::to_string(
        &versions
            .iter()
            .map(|row| {
                json!({
                    "version": row.version,
                    "prerelease": row.prerelease,
                    "yanked": row.yanked,
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("graph version options are serializable");

    html! {
        zed-dependency-graph
            id="dependency-graph"
            data-mode="package"
            data-org=(org)
            data-package=(name)
            data-version=(selected_version)
            data-private=(is_private)
            data-versions=(versions_json)
            data-scope-title="Package dependency graph"
            data-scope-description="Explore declared dependencies, expand neighboring package manifests, run graph queries, and download the same semantic graph in every supported representation." {
            p class="muted" { "Loading the interactive dependency graph…" }
            noscript {
                p class="muted" {
                    "JavaScript is required for the interactive canvas. The canonical representations remain available:"
                }
                table class="dg-fallback-table" {
                    caption { "Dependency graph downloads for " (org) "/" (name) "@" (selected_version) }
                    thead {
                        tr { th scope="col" { "Representation" } th scope="col" { "Download" } }
                    }
                    tbody {
                        @for (format, label) in FALLBACK_EXPORTS {
                            tr {
                                th scope="row" { (label) }
                                td {
                                    a href=(package_export_url(org, name, selected_version, format)) download="" {
                                        @if *format == "csv" { "Open semantic relationship table" } @else { "Download" }
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

fn package_export_url(org: &str, name: &str, version: &str, format: &str) -> String {
    format!(
        "/bff/dependency-graphs/packages/{}/{}/{}/export/{}",
        uri_segment(org),
        uri_segment(name),
        uri_segment(version),
        uri_segment(format),
    )
}

fn uri_segment(value: &str) -> String {
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

pub fn scope_workspace(
    scope_kind: &str,
    title: &str,
    description: &str,
    packages: &[PackageRow],
) -> Markup {
    let sources_json = serde_json::to_string(
        &packages
            .iter()
            .filter_map(|package| {
                package.latest.as_ref().map(|version| {
                    json!({
                        "org": package.org,
                        "name": package.name,
                        "version": version,
                        "private": package.visibility != "public",
                    })
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("graph package sources are serializable");

    html! {
        zed-dependency-graph
            id="dependency-graph"
            data-mode="scope"
            data-scope-kind=(scope_kind)
            data-scope-title=(title)
            data-scope-description=(description)
            data-sources=(sources_json) {
            p class="muted" { "Loading package topology…" }
            noscript {
                p class="muted" {
                    "JavaScript is required to compose the interactive topology. Each package graph remains available independently:"
                }
                table class="dg-fallback-table" {
                    caption { (title) " package sources" }
                    thead {
                        tr { th scope="col" { "Package" } th scope="col" { "Version" } th scope="col" { "Graph" } th scope="col" { "Relationship table" } }
                    }
                    tbody {
                        @for package in packages {
                            @if let Some(version) = &package.latest {
                                tr {
                                    th scope="row" { (package.org) "/" (package.name) }
                                    td class="mono" { (version) }
                                    td { a href=(package_export_url(&package.org, &package.name, version, "json")) download="" { "JSON" } }
                                    td { a href=(package_export_url(&package.org, &package.name, version, "csv")) download="" { "CSV" } }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_workspace_carries_prerelease_and_yank_metadata() {
        let markup = package_workspace(
            "acme",
            "http",
            "2.0.0-beta.1",
            &[GraphVersion {
                version: "2.0.0-beta.1".into(),
                prerelease: true,
                yanked: false,
            }],
            true,
        )
        .into_string();
        assert!(markup.contains("zed-dependency-graph"));
        assert!(markup.contains("2.0.0-beta.1"));
        assert!(markup.contains("&quot;prerelease&quot;:true"));
        assert!(markup.contains("data-private=\"true\""));
        assert!(markup.contains("Dependency graph downloads"));
        assert!(markup.contains("dg-fallback-table"));
        assert!(markup.contains("Open semantic relationship table"));
        assert!(
            markup.contains("/bff/dependency-graphs/packages/acme/http/2.0.0-beta.1/export/yaml")
        );
    }

    #[test]
    fn fallback_exports_encode_every_route_segment() {
        assert_eq!(
            package_export_url("acme tools", "http/client", "1.0.0+build", "json"),
            "/bff/dependency-graphs/packages/acme%20tools/http%2Fclient/1.0.0%2Bbuild/export/json"
        );
    }

    #[test]
    fn scope_workspace_uses_only_published_package_versions() {
        let packages = vec![
            PackageRow {
                org: "acme".into(),
                name: "a".into(),
                description: None,
                latest: Some("1.0.0".into()),
                visibility: "public".into(),
            },
            PackageRow {
                org: "acme".into(),
                name: "empty".into(),
                description: None,
                latest: None,
                visibility: "private".into(),
            },
        ];
        let markup =
            scope_workspace("organization", "Topology", "Description", &packages).into_string();
        assert!(markup.contains("&quot;name&quot;:&quot;a&quot;"));
        assert!(markup.contains("&quot;private&quot;:false"));
        assert!(!markup.contains("&quot;name&quot;:&quot;empty&quot;"));
        assert!(markup.contains("package sources"));
        assert!(markup.contains("/bff/dependency-graphs/packages/acme/a/1.0.0/export/csv"));
    }
}
