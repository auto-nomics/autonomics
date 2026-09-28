//! Panel-bundle provisioning for installed plugins.
//!
//! Every `[[panels]]` entry references a Hugging Face dataset repository
//! (`owner/name`). The daemon resolves those references exclusively through
//! the local verified catalog cache — a bundle that is not installed when
//! the VFS and DAG bundle registry are built can never satisfy a node
//! build.
//!
//! Two independent phases live here. [`check_panel_bundles`] is the
//! startup preflight's local-only presence check — fast, bounded, and
//! offline-safe, so daemon readiness never waits on the network.
//! [`sync_panel_bundles`] is the explicit provisioning phase
//! (`autonomics panels sync`): for each declared bundle it checks the
//! local cache first (no network), then downloads and checksum-verifies
//! the current entry from the configured remote catalog.
//!
//! Installs land in the shared cache root (`AUTONOMICS_PANEL_CACHE_ROOT`,
//! default `~/.autonomics/panels`) that the daemon's local catalog and
//! container panel cache both read, so a download completed here is
//! visible to the daemon after a restart. Failures are reported per
//! bundle instead of aborting: the affected nodes fail closed at build
//! time exactly as they would without this phase.

use std::collections::BTreeSet;

use data_catalog::{LocalCatalog, RemoteCatalog};
use futures::StreamExt as _;

use crate::factory::Plugin;

/// Result of provisioning one panel bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelBundleOutcome {
    /// The current entry is present in the local verified cache.
    Cached,
    /// The current entry was downloaded, checksum-verified, and installed.
    Installed { version: String },
    /// Not present locally, and not fetched — either a check-only pass or
    /// a sync run without a configured remote catalog.
    Missing,
    /// The download attempt failed (network, missing repository, or
    /// checksum mismatch).
    Failed(String),
}

impl PanelBundleOutcome {
    #[cfg(test)]
    fn is_installed(&self) -> bool {
        matches!(self, PanelBundleOutcome::Installed { .. })
    }
}

/// One bundle's provisioning result, keyed by its HF repository id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelBundleStatus {
    pub repo: String,
    pub outcome: PanelBundleOutcome,
}

/// Aggregate counts over a [`PanelBundleReport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelBundleCounts {
    pub cached: usize,
    pub installed: usize,
    /// Missing plus failed downloads: bundles the daemon starts without.
    pub unavailable: usize,
}

/// Bundle results in first-declaration order: [`check_panel_bundles`] and
/// [`sync_panel_bundles`] each return one entry per unique bundle.
#[derive(Debug, Clone, Default)]
pub struct PanelBundleReport {
    pub statuses: Vec<PanelBundleStatus>,
}

/// Streaming progress from [`sync_panel_bundles_with_progress`], delivered
/// as it happens (completion order, not declaration order) so callers can
/// surface per-bundle CLI output during long downloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelBundleEvent<'a> {
    /// A missing bundle's download just started (emitted once per attempt,
    /// before any network fetch for it).
    DownloadStarted { repo: &'a str },
    /// Live byte accounting for an in-flight download; arrives in bursts
    /// (one tick per 4 MiB chunk per file), so callers throttle rendering.
    Progress {
        repo: &'a str,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    /// One bundle finished, with any outcome. Cached bundles produce this
    /// without a preceding [`PanelBundleEvent::DownloadStarted`].
    Finished(&'a PanelBundleStatus),
}

impl PanelBundleReport {
    pub fn is_empty(&self) -> bool {
        self.statuses.is_empty()
    }

    /// The repo ids of every bundle that is not usable (missing or failed).
    pub fn missing_repos(&self) -> Vec<&str> {
        self.statuses
            .iter()
            .filter(|status| status.outcome != PanelBundleOutcome::Cached)
            .filter(|status| !matches!(status.outcome, PanelBundleOutcome::Installed { .. }))
            .map(|status| status.repo.as_str())
            .collect()
    }

    pub fn counts(&self) -> PanelBundleCounts {
        let mut counts = PanelBundleCounts::default();
        for status in &self.statuses {
            match &status.outcome {
                PanelBundleOutcome::Cached => counts.cached += 1,
                PanelBundleOutcome::Installed { .. } => counts.installed += 1,
                PanelBundleOutcome::Missing | PanelBundleOutcome::Failed(_) => {
                    counts.unavailable += 1;
                }
            }
        }
        counts
    }
}

/// Collect the panel bundle repository ids declared by the given plugins,
/// deduplicated across families and in first-declaration order.
pub fn collect_panel_bundles(plugins: &[Plugin]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut bundles = Vec::new();
    for plugin in plugins {
        for panel in plugin.panels() {
            if seen.insert(panel.bundle.to_string()) {
                bundles.push(panel.bundle.to_string());
            }
        }
    }
    bundles
}

/// Default number of bundles downloaded concurrently. Each install is
/// itself parallel over payload files (see `LocalCatalog::stage_entry`),
/// so this bounds the total number of in-flight requests at
/// `BUNDLE_CONCURRENCY × stage-file concurrency`.
pub const BUNDLE_CONCURRENCY: usize = 4;

/// Local-only presence check for the startup preflight: every bundle is
/// resolved against the verified cache with zero network access, so the
/// daemon becomes ready in bounded time regardless of network condition.
/// Missing bundles are reported, not fetched — provisioning is the
/// explicit [`sync_panel_bundles`] phase.
pub fn check_panel_bundles(bundles: &[String], local: &LocalCatalog) -> PanelBundleReport {
    PanelBundleReport {
        statuses: bundles
            .iter()
            .map(|repo| PanelBundleStatus {
                repo: repo.clone(),
                outcome: if local.is_repository_installed(repo) {
                    PanelBundleOutcome::Cached
                } else {
                    PanelBundleOutcome::Missing
                },
            })
            .collect(),
    }
}

/// Ensure every bundle in `bundles` (HF repo ids) is present in the local
/// catalog cache, downloading the current entry from `remote` when needed.
///
/// Cached bundles are checked locally only, so a fully provisioned
/// deployment completes without network access. Bundles are provisioned
/// concurrently ([`BUNDLE_CONCURRENCY`] at a time); each is independent,
/// so one failure never blocks the others. The report preserves the input
/// (first-declaration) order regardless of completion order.
pub async fn sync_panel_bundles(
    bundles: &[String],
    remote: Option<&RemoteCatalog>,
    local: &LocalCatalog,
) -> PanelBundleReport {
    sync_panel_bundles_with_concurrency(bundles, remote, local, BUNDLE_CONCURRENCY).await
}

/// [`sync_panel_bundles`] with an explicit concurrency bound; `1`
/// reproduces the historical serial behavior.
pub async fn sync_panel_bundles_with_concurrency(
    bundles: &[String],
    remote: Option<&RemoteCatalog>,
    local: &LocalCatalog,
    concurrency: usize,
) -> PanelBundleReport {
    sync_panel_bundles_with_progress(bundles, remote, local, concurrency, &|_| {}).await
}

/// [`sync_panel_bundles`] with an explicit concurrency bound plus a
/// progress callback. Events arrive in completion order as the run
/// progresses — the caller can stream them straight to CLI output instead
/// of waiting for the batched report.
pub async fn sync_panel_bundles_with_progress(
    bundles: &[String],
    remote: Option<&RemoteCatalog>,
    local: &LocalCatalog,
    concurrency: usize,
    on_event: &(dyn Fn(PanelBundleEvent<'_>) + Send + Sync),
) -> PanelBundleReport {
    let mut indexed: Vec<(usize, PanelBundleStatus)> =
        futures::stream::iter(bundles.iter().enumerate())
            .map(|(index, repo)| async move {
                let outcome = ensure_bundle(repo, remote, local, on_event).await;
                let status = PanelBundleStatus {
                    repo: repo.clone(),
                    outcome,
                };
                on_event(PanelBundleEvent::Finished(&status));
                (index, status)
            })
            .buffer_unordered(concurrency.max(1))
            .collect()
            .await;
    indexed.sort_by_key(|(index, _)| *index);
    PanelBundleReport {
        statuses: indexed.into_iter().map(|(_, status)| status).collect(),
    }
}

async fn ensure_bundle(
    repo: &str,
    remote: Option<&RemoteCatalog>,
    local: &LocalCatalog,
    on_event: &(dyn Fn(PanelBundleEvent<'_>) + Send + Sync),
) -> PanelBundleOutcome {
    if local.is_repository_installed(repo) {
        return PanelBundleOutcome::Cached;
    }
    match remote {
        None => PanelBundleOutcome::Missing,
        Some(remote) => {
            on_event(PanelBundleEvent::DownloadStarted { repo });
            // Forward the catalog layer's byte ticks as Progress events so
            // the CLI can render live speed and completion percentages.
            let sink_repo = repo.to_string();
            let sink: data_catalog::ProgressSink<'_> = std::sync::Arc::new(move |tick| {
                on_event(PanelBundleEvent::Progress {
                    repo: &sink_repo,
                    downloaded_bytes: tick.downloaded_bytes,
                    total_bytes: tick.total_bytes,
                });
            });
            match local
                .install_repository_with_progress(remote, repo, Some(sink))
                .await
            {
                Ok(entry) => PanelBundleOutcome::Installed {
                    version: entry.version,
                },
                Err(error) => PanelBundleOutcome::Failed(error.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::PluginManifest;
    use data_catalog::remote::test_utils::MapSource;
    use std::sync::Arc;

    fn plugin_with_panels(name: &str, bundles: &[&str]) -> Plugin {
        let panels: String = bundles
            .iter()
            .enumerate()
            .map(|(index, bundle)| {
                format!(
                    "\n[[panels]]\nbinding = \"p{index}\"\nmount = \"/panels/p{index}\"\nbundle = \"{bundle}\"\n"
                )
            })
            .collect();
        let manifest: PluginManifest = toml::from_str(&format!(
            r#"
schema_version = 1
plugin_name = "{name}"

[image]
reference = "ghcr.io/auto-nomics/autonomics/test@sha256:{}"

{panels}

[[nodes]]
kind = "{name}_node"
desc = "d"
doc = "doc"

[nodes.ports]
inputs = [{{ type = "file" }}]
outputs = [{{ path = "out.log", format = "text" }}]

[nodes.command]
interpreter = "sh"
script = "true"
"#,
            "a".repeat(64)
        ))
        .expect("fixture manifest parses");
        let state = tempfile::tempdir().unwrap();
        let runtime: Arc<dyn container_runtime::PodmanConnection> = Arc::new(
            container_runtime::PodmanRuntime::new(container_runtime::PodmanConfig {
                program: "podman".into(),
                workspace_root: state.path().join("workspace"),
                panel_cache_root: state.path().join("panels"),
            }),
        );
        let panel_cache = Arc::new(container_runtime::PanelCache::new(
            state.path().join("panels"),
        ));
        Plugin::new(manifest, runtime, panel_cache)
    }

    /// Build a remote catalog whose package repositories each hold one
    /// current verified entry with a single payload file.
    async fn remote_with_bundles(repos: &[&str]) -> RemoteCatalog {
        let mut objects = MapSource::default();
        for repo in repos {
            let workspace = tempfile::tempdir().unwrap();
            let input = workspace.path().join("input");
            std::fs::create_dir_all(&input).unwrap();
            std::fs::write(
                input.join("data.txt"),
                format!("{repo} payload").into_bytes(),
            )
            .unwrap();
            let package = data_catalog::build_package(
                &input,
                workspace.path().join("package"),
                data_catalog::package::BuildOptions {
                    repo: Some((*repo).into()),
                    version: Some("v1".into()),
                    kind: Some("table".into()),
                    ..Default::default()
                },
            )
            .unwrap();
            let manifest = package.manifest;
            let entry = data_catalog::CatalogEntry {
                repo: data_catalog::HfRepoId::new(repo).unwrap(),
                version: manifest.version.clone(),
                kind: manifest.kind.clone(),
                digest: manifest.digest.clone().expect("fixture has digest"),
                current: true,
                created_unix_seconds: 1,
            };
            let package_index = data_catalog::CatalogIndex {
                entries: vec![entry.clone()],
                ..data_catalog::CatalogIndex::default()
            };
            objects.0.insert(
                entry.source_manifest_key(),
                serde_json::to_vec(&manifest).unwrap(),
            );
            objects.0.insert(
                entry.source_payload_path("data.txt"),
                format!("{repo} payload").into_bytes(),
            );
            objects.0.insert(
                format!("{repo}/index.json"),
                serde_json::to_vec(&package_index).unwrap(),
            );
        }
        RemoteCatalog::from_source(
            data_catalog::CatalogConfig {
                repository: Some("owner/catalog-index".into()),
                ..data_catalog::CatalogConfig::default()
            },
            Box::new(objects),
        )
    }

    fn bundle_ids(repos: &[&str]) -> Vec<String> {
        repos.iter().map(|repo| repo.to_string()).collect()
    }

    #[test]
    fn collect_dedups_across_families_in_declaration_order() {
        let plugins = vec![
            plugin_with_panels("ldsc", &["owner/shared", "owner/first"]),
            plugin_with_panels("mtag", &["owner/shared", "owner/second"]),
        ];
        assert_eq!(
            collect_panel_bundles(&plugins),
            vec!["owner/shared", "owner/first", "owner/second"]
        );
    }

    #[tokio::test]
    async fn installs_missing_bundles_then_reports_cached_without_network() {
        let remote = remote_with_bundles(&["owner/bundle-a", "owner/bundle-b"]).await;
        let cache_root = tempfile::tempdir().unwrap();
        let local = LocalCatalog::open(cache_root.path()).unwrap();

        let report = sync_panel_bundles(
            &bundle_ids(&["owner/bundle-a", "owner/bundle-b"]),
            Some(&remote),
            &local,
        )
        .await;
        assert_eq!(
            report.statuses[0].outcome,
            PanelBundleOutcome::Installed {
                version: "v1".into()
            }
        );
        assert_eq!(
            report.statuses[1].outcome,
            PanelBundleOutcome::Installed {
                version: "v1".into()
            }
        );
        assert_eq!(local.index().unwrap().current_entries().count(), 2);

        // Second run: everything cached, and no remote is passed at all —
        // proving the cached path never needs network access.
        let again = sync_panel_bundles(
            &bundle_ids(&["owner/bundle-a", "owner/bundle-b"]),
            None,
            &local,
        )
        .await;
        assert_eq!(again.statuses[0].outcome, PanelBundleOutcome::Cached);
        assert_eq!(again.statuses[1].outcome, PanelBundleOutcome::Cached);
    }

    #[tokio::test]
    async fn check_panel_bundles_is_local_only_and_names_missing_repos() {
        let remote = remote_with_bundles(&["owner/present"]).await;
        let cache_root = tempfile::tempdir().unwrap();
        let local = LocalCatalog::open(cache_root.path()).unwrap();
        local
            .install_repository(&remote, "owner/present")
            .await
            .unwrap();

        // No remote is involved at all — the check must resolve purely
        // against the local cache.
        let report = check_panel_bundles(&bundle_ids(&["owner/absent", "owner/present"]), &local);
        assert_eq!(report.statuses[0].outcome, PanelBundleOutcome::Missing);
        assert_eq!(report.statuses[1].outcome, PanelBundleOutcome::Cached);
        assert_eq!(report.missing_repos(), vec!["owner/absent"]);
    }

    #[tokio::test]
    async fn missing_bundle_without_remote_is_reported_not_fatal() {
        let cache_root = tempfile::tempdir().unwrap();
        let local = LocalCatalog::open(cache_root.path()).unwrap();

        let report = sync_panel_bundles(&bundle_ids(&["owner/never"]), None, &local).await;
        assert_eq!(report.statuses[0].outcome, PanelBundleOutcome::Missing);
        assert_eq!(
            report.counts(),
            PanelBundleCounts {
                cached: 0,
                installed: 0,
                unavailable: 1
            }
        );
    }

    #[tokio::test]
    async fn failed_download_does_not_block_other_bundles() {
        let remote = remote_with_bundles(&["owner/good"]).await;
        let cache_root = tempfile::tempdir().unwrap();
        let local = LocalCatalog::open(cache_root.path()).unwrap();

        let report = sync_panel_bundles(
            &bundle_ids(&["owner/good", "owner/absent"]),
            Some(&remote),
            &local,
        )
        .await;
        assert_eq!(
            report.statuses[0].outcome,
            PanelBundleOutcome::Installed {
                version: "v1".into()
            }
        );
        assert!(matches!(
            report.statuses[1].outcome,
            PanelBundleOutcome::Failed(_)
        ));
        assert_eq!(
            report.counts(),
            PanelBundleCounts {
                cached: 0,
                installed: 1,
                unavailable: 1
            }
        );
    }

    #[tokio::test]
    async fn parallel_sync_preserves_declaration_order_in_the_report() {
        // Concurrent installs complete out of order (the later bundle is
        // tiny, the earlier one is larger); the report must still list
        // them in first-declaration order.
        let remote = remote_with_bundles(&["owner/first", "owner/second", "owner/third"]).await;
        let cache_root = tempfile::tempdir().unwrap();
        let local = LocalCatalog::open(cache_root.path()).unwrap();

        let report = sync_panel_bundles_with_concurrency(
            &bundle_ids(&["owner/first", "owner/second", "owner/third"]),
            Some(&remote),
            &local,
            3,
        )
        .await;
        let repos: Vec<&str> = report.statuses.iter().map(|s| s.repo.as_str()).collect();
        assert_eq!(repos, vec!["owner/first", "owner/second", "owner/third"]);
        assert!(report.statuses.iter().all(|s| s.outcome.is_installed()));
    }

    #[tokio::test]
    async fn progress_events_stream_starts_and_finishes_in_completion_order() {
        let remote = remote_with_bundles(&["owner/fresh"]).await;
        let cache_root = tempfile::tempdir().unwrap();
        let local = LocalCatalog::open(cache_root.path()).unwrap();
        // Pre-install one bundle so its finish arrives without a start.
        local
            .install_repository(&remote, "owner/fresh")
            .await
            .unwrap();

        let events: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let recorder = events.clone();
        let record = move |event: PanelBundleEvent<'_>| match event {
            PanelBundleEvent::DownloadStarted { repo } => {
                recorder.lock().unwrap().push(format!("start:{repo}"))
            }
            PanelBundleEvent::Progress {
                repo,
                downloaded_bytes,
                total_bytes,
            } => recorder
                .lock()
                .unwrap()
                .push(format!("bytes:{repo}:{downloaded_bytes}/{total_bytes}")),
            PanelBundleEvent::Finished(status) => recorder
                .lock()
                .unwrap()
                .push(format!("finish:{}", status.repo)),
        };

        // Both bundles are already cached after the pre-install above;
        // install a second, still-missing bundle by declaring one the
        // fixture does not pre-install.
        let remote_two = remote_with_bundles(&["owner/second"]).await;
        sync_panel_bundles_with_progress(
            &bundle_ids(&["owner/fresh", "owner/second"]),
            Some(&remote_two),
            &local,
            2,
            &record,
        )
        .await;

        // Concurrent futures interleave freely across bundles, so only
        // per-bundle properties are asserted.
        let events = events.lock().unwrap().clone();
        assert!(
            !events.contains(&"start:owner/fresh".to_string()),
            "cached bundles must not emit start events: {events:?}"
        );
        assert!(events.contains(&"finish:owner/fresh".to_string()));
        let start = events
            .iter()
            .position(|event| event == "start:owner/second")
            .expect("missing bundle emits a start event");
        let finish = events
            .iter()
            .position(|event| event == "finish:owner/second")
            .expect("missing bundle emits a finish event");
        assert!(start < finish, "start must precede finish: {events:?}");
        let payload_len = "owner/second payload".len() as u64;
        assert!(
            events.contains(&format!("bytes:owner/second:{payload_len}/{payload_len}")),
            "byte progress must reach the payload size: {events:?}"
        );
    }
}
