//! The storage console.
//!
//! A portal onto whatever object store is configured, not a window onto one
//! vendor's dashboard. Everything rendered here comes from the registry API's
//! own provider-agnostic report, so the page is identical whether the bytes are
//! in Cloudflare R2, S3, Google Cloud Storage, MinIO, a directory, or process
//! memory — and stays identical the day the bucket moves.
//!
//! Rendering is a pure function of the view model. The fetch, the session
//! rotation, and the error handling all happen in [`crate::routes::storage`].

use maud::{Markup, html};

use super::components::human_size;

/// Whether the backend answered, mirroring the API's health sum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    Reachable { latency_ms: u64 },
    Unreachable { reason: String },
    Unprobed,
}

/// Everything the console shows about the configured backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageView {
    pub provider_label: String,
    pub provider_slug: String,
    pub kind: String,
    pub durable: bool,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub endpoint_host: Option<String>,
    pub path_style: Option<bool>,
    pub directory: Option<String>,
    pub health: Health,
    pub artifact_count: u64,
    pub total_bytes: u64,
    pub largest_bytes: u64,
    pub max_artifact_bytes: u64,
    pub observed_at: String,
}

/// One reconciled artifact, mirroring the API's three-state verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectVerdict {
    Consistent,
    Divergent { detail: String },
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectView {
    pub key: String,
    pub sha256: String,
    pub recorded_bytes: u64,
    pub stored_bytes: Option<u64>,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
    pub last_modified: Option<String>,
    pub verdict: ObjectVerdict,
}

/// Percentage of the upload ceiling the largest stored artifact occupies.
///
/// Clamped rather than allowed to exceed 100: an artifact stored before the
/// ceiling was lowered is legitimate history, and a meter that overflows its
/// track reads as a rendering bug rather than as the fact it is reporting.
#[must_use]
pub fn ceiling_fraction(largest_bytes: u64, max_artifact_bytes: u64) -> u32 {
    if max_artifact_bytes == 0 {
        return 0;
    }
    let ratio = (largest_bytes as f64 / max_artifact_bytes as f64) * 100.0;
    ratio.clamp(0.0, 100.0).round() as u32
}

fn health_pill(health: &Health) -> Markup {
    html! {
        @match health {
            Health::Reachable { latency_ms } => {
                span class="status-pill status-ok" {
                    span aria-hidden="true" { "●" }
                    span { "reachable · " (latency_ms) " ms" }
                }
            },
            Health::Unreachable { reason } => {
                span class="status-pill status-bad" title=(reason) {
                    span aria-hidden="true" { "✕" }
                    span { "unreachable" }
                }
            },
            Health::Unprobed => {
                span class="status-pill status-idle" {
                    span aria-hidden="true" { "○" }
                    span { "not probed" }
                }
            },
        }
    }
}

fn verdict_pill(verdict: &ObjectVerdict) -> Markup {
    html! {
        @match verdict {
            ObjectVerdict::Consistent => {
                span class="status-pill status-ok" {
                    span aria-hidden="true" { "●" }
                    span { "matches the registry" }
                }
            },
            ObjectVerdict::Divergent { detail } => {
                span class="status-pill status-warn" title=(detail) {
                    span aria-hidden="true" { "▲" }
                    span { "diverged" }
                }
            },
            ObjectVerdict::Missing => {
                span class="status-pill status-bad" {
                    span aria-hidden="true" { "✕" }
                    span { "missing from the store" }
                }
            },
        }
    }
}

/// The whole page body.
pub fn page(view: &StorageView, object: Option<&ObjectResult>) -> Markup {
    html! {
        h1 { "Storage" }
        p class="lede" {
            "Where published artifacts actually live. Zed addresses every backend "
            "through the same S3-compatible contract, so this page reads the same "
            "whether the bucket is on Cloudflare R2, S3, Google Cloud Storage, or a "
            "disk in the next room."
        }

        div class="kpi-row" {
            div class="kpi" {
                span class="kpi-label" { "Backend" }
                span class="kpi-value" { (view.provider_label) }
                span class="kpi-note" {
                    (view.kind)
                    @if !view.durable { " · not durable" }
                }
            }
            div class="kpi" {
                span class="kpi-label" { "Artifacts" }
                span class="kpi-value" { (view.artifact_count) }
                span class="kpi-note" { "distinct digests" }
            }
            div class="kpi" {
                span class="kpi-label" { "Stored" }
                span class="kpi-value" { (human_size(view.total_bytes as i64)) }
                span class="kpi-note" { "as recorded by the registry" }
            }
            div class="kpi" {
                span class="kpi-label" { "Largest artifact" }
                span class="kpi-value" { (human_size(view.largest_bytes as i64)) }
                span class="kpi-note" {
                    "ceiling " (human_size(view.max_artifact_bytes as i64))
                }
                div class="meter"
                    role="img"
                    aria-label={
                        "largest artifact is "
                        (ceiling_fraction(view.largest_bytes, view.max_artifact_bytes))
                        "% of the upload ceiling"
                    } {
                    span style={
                        "width:" (ceiling_fraction(view.largest_bytes, view.max_artifact_bytes)) "%"
                    } {}
                }
            }
        }

        section class="card" {
            h2 { "Backend" }
            p { (health_pill(&view.health)) }
            @if let Health::Unreachable { reason } = &view.health {
                p class="warn" { (reason) }
            }
            @if !view.durable {
                p class="warn" {
                    "This backend does not survive a restart. Anything published "
                    "here is disposable — never run a real registry on it."
                }
            }
            dl class="kv" {
                dt { "provider" } dd { (view.provider_slug) }
                dt { "kind" } dd { (view.kind) }
                @if let Some(bucket) = &view.bucket {
                    dt { "bucket" } dd { (bucket) }
                }
                @if let Some(region) = &view.region {
                    dt { "region" } dd { (region) }
                }
                @if let Some(host) = &view.endpoint_host {
                    dt { "endpoint" } dd { (host) }
                }
                @if let Some(path_style) = view.path_style {
                    dt { "addressing" }
                    dd { @if path_style { "path-style" } @else { "virtual-hosted" } }
                }
                @if let Some(directory) = &view.directory {
                    dt { "directory" } dd { (directory) }
                }
                dt { "observed" } dd { (view.observed_at) }
            }
            p class="muted" {
                "Credentials are never read by this page. The registry API reports "
                "only the bucket and host, which are configuration you already have."
            }
        }

        section class="card" id="reconcile" {
            h2 { "Reconcile an artifact" }
            p class="lede" {
                "Check that the store still holds what the registry promises. Paste "
                "a published artifact's sha256 — every version page lists one."
            }
            form class="stack" method="get" action="/console/storage#reconcile" {
                label for="artifact" { "artifact sha256" }
                input id="artifact"
                      name="artifact"
                      class="mono"
                      type="text"
                      inputmode="latin"
                      autocomplete="off"
                      spellcheck="false"
                      minlength="64"
                      maxlength="64"
                      pattern="[0-9a-f]{64}"
                      placeholder="64 lowercase hex characters";
                button type="submit" class="button primary" { "Reconcile" }
            }

            @match object {
                Some(ObjectResult::Found(object)) => { (object_card(object)) },
                Some(ObjectResult::NotPublished { digest }) => {
                    p class="warn" {
                        "No published version records " span class="mono" { (digest) } "."
                    }
                },
                Some(ObjectResult::Invalid) => {
                    p class="warn" {
                        "An artifact digest is exactly 64 lowercase hex characters."
                    }
                },
                Some(ObjectResult::BackendUnavailable { reason }) => {
                    p class="warn" { "The store could not be asked: " (reason) }
                },
                None => {},
            }
        }
    }
}

/// The outcome of a reconciliation request, so the page renders one of a
/// closed set of states rather than an optional object beside an optional
/// error that could both be present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectResult {
    Found(ObjectView),
    NotPublished { digest: String },
    Invalid,
    BackendUnavailable { reason: String },
}

fn object_card(object: &ObjectView) -> Markup {
    html! {
        div class="card" {
            p { (verdict_pill(&object.verdict)) }
            @if let ObjectVerdict::Divergent { detail } = &object.verdict {
                p class="warn" { (detail) }
            }
            dl class="kv" {
                dt { "key" } dd { (object.key) }
                dt { "sha256" } dd { (object.sha256) }
                dt { "registry size" } dd { (human_size(object.recorded_bytes as i64)) }
                @if let Some(stored) = object.stored_bytes {
                    dt { "stored size" } dd { (human_size(stored as i64)) }
                }
                @if let Some(content_type) = &object.content_type {
                    dt { "content type" } dd { (content_type) }
                }
                @if let Some(cache_control) = &object.cache_control {
                    dt { "cache control" } dd { (cache_control) }
                }
                @if let Some(modified) = &object.last_modified {
                    dt { "last modified" } dd { (modified) }
                }
            }
        }
    }
}

/// Shown when the console cannot reach the registry API at all — distinct from
/// the API reporting that *storage* is down, which is a finding rather than a
/// failure.
pub fn unavailable(reason: &str) -> Markup {
    html! {
        h1 { "Storage" }
        div class="empty" {
            p { "The registry API did not answer, so the storage backend cannot be described." }
            p class="muted mono" { (reason) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> StorageView {
        StorageView {
            provider_label: "Cloudflare R2".into(),
            provider_slug: "cloudflare-r2".into(),
            kind: "object-store".into(),
            durable: true,
            bucket: Some("zed-artifacts".into()),
            region: Some("auto".into()),
            endpoint_host: Some("abc123.r2.cloudflarestorage.com".into()),
            path_style: Some(false),
            directory: None,
            health: Health::Reachable { latency_ms: 37 },
            artifact_count: 1284,
            total_bytes: 9_317_842_944,
            largest_bytes: 73_400_320,
            max_artifact_bytes: 104_857_600,
            observed_at: "2026-08-23T18:42:11Z".into(),
        }
    }

    #[test]
    fn the_page_names_the_live_provider() {
        let markup = page(&view(), None).into_string();
        assert!(markup.contains("Cloudflare R2"), "{markup}");
        assert!(markup.contains("abc123.r2.cloudflarestorage.com"));
        assert!(markup.contains("reachable"));
    }

    #[test]
    fn the_page_never_embeds_a_vendor_console() {
        let markup = page(&view(), None).into_string();
        // The whole point of the portal: no iframe, and no link that would
        // only work for one provider.
        assert!(!markup.contains("<iframe"), "{markup}");
        assert!(!markup.contains("dash.cloudflare.com"), "{markup}");
        assert!(!markup.contains("console.aws.amazon.com"), "{markup}");
    }

    #[test]
    fn a_non_durable_backend_is_called_out() {
        let mut memory = view();
        memory.durable = false;
        memory.provider_label = "Process memory".into();
        let markup = page(&memory, None).into_string();
        assert!(markup.contains("does not survive a restart"), "{markup}");
    }

    #[test]
    fn an_unreachable_backend_shows_its_reason_and_no_latency() {
        let mut down = view();
        down.health = Health::Unreachable {
            reason: "connection refused".into(),
        };
        let markup = page(&down, None).into_string();
        assert!(markup.contains("unreachable"), "{markup}");
        assert!(markup.contains("connection refused"), "{markup}");
        assert!(!markup.contains(" ms"), "{markup}");
    }

    #[test]
    fn the_ceiling_meter_never_overflows_its_track() {
        assert_eq!(ceiling_fraction(0, 100), 0);
        assert_eq!(ceiling_fraction(50, 100), 50);
        assert_eq!(ceiling_fraction(100, 100), 100);
        // An artifact stored before the ceiling was lowered.
        assert_eq!(ceiling_fraction(500, 100), 100);
        // A ceiling of zero must not divide.
        assert_eq!(ceiling_fraction(10, 0), 0);
    }

    #[test]
    fn each_reconciliation_verdict_renders_distinctly() {
        let base = ObjectView {
            key: "artifacts/abc.tar.gz".into(),
            sha256: "abc".into(),
            recorded_bytes: 4096,
            stored_bytes: Some(4096),
            content_type: Some("application/gzip".into()),
            cache_control: None,
            last_modified: None,
            verdict: ObjectVerdict::Consistent,
        };
        let consistent = page(&view(), Some(&ObjectResult::Found(base.clone()))).into_string();
        assert!(consistent.contains("matches the registry"));

        let missing = ObjectView {
            stored_bytes: None,
            verdict: ObjectVerdict::Missing,
            ..base.clone()
        };
        let markup = page(&view(), Some(&ObjectResult::Found(missing))).into_string();
        assert!(markup.contains("missing from the store"), "{markup}");
        assert!(!markup.contains("matches the registry"), "{markup}");

        let diverged = ObjectView {
            verdict: ObjectVerdict::Divergent {
                detail: "registry records 4096 bytes; store reports 9".into(),
            },
            ..base
        };
        let markup = page(&view(), Some(&ObjectResult::Found(diverged))).into_string();
        assert!(markup.contains("diverged"), "{markup}");
        assert!(markup.contains("store reports 9"), "{markup}");
    }

    #[test]
    fn an_unpublished_digest_is_reported_without_probing_the_bucket() {
        let markup = page(
            &view(),
            Some(&ObjectResult::NotPublished {
                digest: "a".repeat(64),
            }),
        )
        .into_string();
        assert!(markup.contains("No published version records"), "{markup}");
    }

    #[test]
    fn the_api_being_down_is_distinct_from_storage_being_down() {
        let markup = unavailable("502 from the registry API").into_string();
        assert!(markup.contains("did not answer"), "{markup}");
        assert!(!markup.contains("unreachable"), "{markup}");
    }
}
