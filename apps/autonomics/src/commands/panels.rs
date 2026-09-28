//! `autonomics panels` implementation — the provisioning half of the
//! panel-bundle lifecycle.
//!
//! Startup preflight only *checks* bundle presence (local, offline-safe);
//! this command is the explicit, network-bound phase that fills the gaps.
//! The two are deliberately independent so daemon readiness never waits
//! on downloads — see `serve.rs::panel_bundle_preflight`.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use color_eyre::Result;

use container_plugin::bundles::{
    self, BUNDLE_CONCURRENCY, PanelBundleEvent, PanelBundleOutcome, PanelBundleReport,
};
use container_plugin::loader;
use data_catalog::{CatalogConfig, LocalCatalog, RemoteCatalog};

use crate::cli::{PanelsAction, PanelsArgs};

pub fn run_panels(args: PanelsArgs) -> Result<()> {
    match args.action.unwrap_or(PanelsAction::Sync) {
        PanelsAction::Sync => {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
            let state_dir = gateway::RuntimeConfig::builder().build().state_dir;
            let result = runtime.block_on(async move { sync_command(&state_dir).await });
            // Bounded shutdown: the HF client's idle keep-alive connections
            // would otherwise pin the runtime and hang the process after
            // the summary has already printed (mirrors `serve`).
            runtime.shutdown_timeout(std::time::Duration::from_secs(5));
            result
        }
    }
}

async fn sync_command(state_dir: &Path) -> Result<()> {
    let report = provision_missing_bundles(state_dir).await?;
    let counts = report.counts();
    if counts.unavailable > 0 {
        let missing = report.missing_repos().join(", ");
        return Err(color_eyre::eyre::eyre!(
            "provisioning incomplete: {} bundle{} unavailable ({missing})",
            counts.unavailable,
            if counts.unavailable == 1 { "" } else { "s" },
        ));
    }
    Ok(())
}

/// Load every installed plugin family from `state_dir/plugins`.
///
/// Shared by the `panels sync` command and `serve`'s preflight so both
/// walk the same manifests. The connection and panel cache constructed
/// here are only carried for factory construction — nothing executes.
pub(crate) fn load_installed_plugins(
    state_dir: &Path,
) -> Result<Vec<container_plugin::factory::Plugin>> {
    let root = state_dir.join("plugins");
    let connection: std::sync::Arc<dyn container_runtime::PodmanConnection> = std::sync::Arc::new(
        container_runtime::PodmanRuntime::new(container_runtime::PodmanConfig {
            program: "podman".into(),
            workspace_root: root.join("workspace"),
            panel_cache_root: root.join("panels"),
        }),
    );
    let panel_cache = std::sync::Arc::new(container_runtime::PanelCache::new(root.join("panels")));
    loader::load(&root, connection, panel_cache)
        .map_err(|error| color_eyre::eyre::eyre!("plugin load failed: {error}"))
}

/// Provision every declared panel bundle with streaming CLI output.
///
/// Also the implementation behind the `AUTONOMICS_PANEL_SYNC=1` inline
/// mode of `autonomics serve` (unattended deployments that want the cache
/// filled before the daemon builds its bundle registry).
pub(crate) async fn provision_missing_bundles(state_dir: &Path) -> Result<PanelBundleReport> {
    let plugins = load_installed_plugins(state_dir)?;
    let bundle_ids = bundles::collect_panel_bundles(&plugins);
    if bundle_ids.is_empty() {
        println!("panels: no data bundles declared by installed plugins");
        return Ok(PanelBundleReport::default());
    }

    let catalog_config = read_catalog_config(state_dir)?;
    let remote = match catalog_config.as_ref().filter(|config| config.enabled) {
        Some(config) => {
            let repository = config.repository.as_deref().ok_or_else(|| {
                color_eyre::eyre::eyre!("catalog is enabled but declares no repository")
            })?;
            Some(
                RemoteCatalog::hf(
                    repository,
                    config.revision.clone(),
                    data_catalog::hf::resolve_hf_token(None),
                )
                .map_err(|error| color_eyre::eyre::eyre!("cannot open remote catalog: {error}"))?,
            )
        }
        None => None,
    };

    let local = LocalCatalog::open(data_catalog::default_panel_cache_root())
        .map_err(|error| color_eyre::eyre::eyre!("cannot open panel cache: {error}"))?;
    println!(
        "panels: checking {} data bundle{} against the catalog cache{}",
        bundle_ids.len(),
        if bundle_ids.len() == 1 { "" } else { "s" },
        match remote {
            Some(_) => "",
            None => " (no catalog configured — nothing can be fetched)",
        }
    );
    // The renderer lives on the stack of this call and is borrowed by the
    // event callback; the mutex gives the `Fn + Send + Sync` callback
    // honest interior mutability over it.
    use std::io::IsTerminal;
    let renderer = std::sync::Mutex::new(ProgressLine::new(std::io::stdout().is_terminal()));
    let report = bundles::sync_panel_bundles_with_progress(
        &bundle_ids,
        remote.as_ref(),
        &local,
        BUNDLE_CONCURRENCY,
        &|event| {
            let mut line = renderer.lock().expect("progress renderer");
            match event {
                PanelBundleEvent::DownloadStarted { repo } => {
                    line.clear_line();
                    println!("  ↓ {repo}");
                    line.note_started(repo);
                }
                PanelBundleEvent::Progress {
                    repo,
                    downloaded_bytes,
                    total_bytes,
                } => line.on_progress(repo, downloaded_bytes, total_bytes),
                PanelBundleEvent::Finished(status) => {
                    line.finish(&status.repo);
                    line.clear_line();
                    match &status.outcome {
                        PanelBundleOutcome::Cached => {}
                        PanelBundleOutcome::Installed { version } => {
                            println!("  + {} ({})", status.repo, version)
                        }
                        PanelBundleOutcome::Missing => println!(
                            "  ! {} not cached and no catalog is configured to fetch it",
                            status.repo
                        ),
                        PanelBundleOutcome::Failed(error) => {
                            println!("  ! {} download failed: {error}", status.repo)
                        }
                    }
                }
            }
        },
    )
    .await;
    renderer.lock().expect("progress renderer").finish_all();

    let counts = report.counts();
    println!(
        "panels: {} data bundle{} — {} cached, {} downloaded, {} unavailable",
        report.statuses.len(),
        if report.statuses.len() == 1 { "" } else { "s" },
        counts.cached,
        counts.installed,
        counts.unavailable
    );
    Ok(report)
}

/// repaint interval for the live progress line; byte ticks arrive per
/// 4 MiB chunk per file, so unthrottled painting would flicker.
const PROGRESS_REPAINT: Duration = Duration::from_millis(200);
/// rolling window backing the speed readout.
const PROGRESS_WINDOW: Duration = Duration::from_secs(3);

/// Single-line, rewrite-in-place progress display for the provisioning
/// run.
///
/// Only paints when stdout is a terminal — piped or daemon-nulled stdout
/// stays clean (no ANSI spam in logs), the `↓`/`+`/`!` lifecycle lines
/// still record everything. Log lines clear the progress line first so
/// the two never interleave.
struct ProgressLine {
    is_tty: bool,
    painted: bool,
    last_paint: Option<Instant>,
    /// repo -> (downloaded, total) for in-flight bundles.
    per_bundle: HashMap<String, (u64, u64)>,
    order: Vec<String>,
    /// (time, aggregate downloaded bytes) samples for the speed readout.
    samples: VecDeque<(Instant, u64)>,
}

impl ProgressLine {
    fn new(is_tty: bool) -> Self {
        Self {
            is_tty,
            painted: false,
            last_paint: None,
            per_bundle: HashMap::new(),
            order: Vec::new(),
            samples: VecDeque::new(),
        }
    }

    fn note_started(&mut self, repo: &str) {
        if !self.order.iter().any(|known| known == repo) {
            self.order.push(repo.to_string());
        }
        self.per_bundle.entry(repo.to_string()).or_insert((0, 0));
    }

    fn on_progress(&mut self, repo: &str, downloaded: u64, total: u64) {
        if !self.is_tty {
            return;
        }
        self.per_bundle
            .insert(repo.to_string(), (downloaded, total));
        let aggregate: u64 = self.per_bundle.values().map(|(done, _)| done).sum();
        let now = Instant::now();
        self.samples.push_back((now, aggregate));
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > PROGRESS_WINDOW)
        {
            self.samples.pop_front();
        }
        if self
            .last_paint
            .is_none_or(|at| now.duration_since(at) >= PROGRESS_REPAINT)
        {
            self.paint();
            self.last_paint = Some(now);
        }
    }

    fn finish(&mut self, repo: &str) {
        self.per_bundle.remove(repo);
        self.order.retain(|known| known != repo);
        self.samples.clear();
    }

    /// Erase the in-place line (after any println) so the next paint or
    /// log line starts from column zero.
    fn clear_line(&mut self) {
        if self.painted {
            print!("\r\x1b[2K");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            self.painted = false;
        }
    }

    fn finish_all(&mut self) {
        self.clear_line();
        self.per_bundle.clear();
        self.order.clear();
        self.samples.clear();
    }

    fn paint(&mut self) {
        let total: u64 = self.per_bundle.values().map(|(_, total)| total).sum();
        let done: u64 = self.per_bundle.values().map(|(done, _)| done).sum();
        let percent = if total > 0 {
            done as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        let per_bundle = self
            .order
            .iter()
            .filter_map(|repo| {
                self.per_bundle
                    .get(repo)
                    .map(|(done, total)| (repo, *done, *total))
            })
            .map(|(repo, done, total)| {
                let name = short_bundle_name(repo);
                let percent = if total > 0 { done * 100 / total } else { 0 };
                format!("{name} {percent}%")
            })
            .collect::<Vec<_>>()
            .join(", ");
        print!(
            "\r\x1b[2K  ⇒ {} downloading | {} / {} ({percent:.0}%) | {}/s | {per_bundle}",
            self.per_bundle.len(),
            fmt_bytes(done),
            fmt_bytes(total),
            fmt_bytes(speed_bytes_per_sec(self.samples.make_contiguous())),
        );
        use std::io::Write;
        let _ = std::io::stdout().flush();
        self.painted = true;
    }
}

/// `owner/catalog-lava-ref-ukb-eur` → `lava-ref-ukb-eur`.
fn short_bundle_name(repo: &str) -> &str {
    let name = repo.rsplit('/').next().unwrap_or(repo);
    name.strip_prefix("catalog-").unwrap_or(name)
}

fn fmt_bytes(bytes: u64) -> String {
    const UNIT: f64 = 1024.0;
    let value = bytes as f64;
    if value >= UNIT * UNIT * UNIT {
        format!("{:.1} GiB", value / (UNIT * UNIT * UNIT))
    } else if value >= UNIT * UNIT {
        format!("{:.1} MiB", value / (UNIT * UNIT))
    } else if value >= UNIT {
        format!("{:.1} KiB", value / UNIT)
    } else {
        format!("{bytes} B")
    }
}

fn speed_bytes_per_sec(samples: &[(Instant, u64)]) -> u64 {
    let (Some((first_at, first_bytes)), Some((last_at, last_bytes))) =
        (samples.first(), samples.last())
    else {
        return 0;
    };
    let elapsed = last_at.duration_since(*first_at).as_secs_f64();
    if elapsed <= f64::EPSILON {
        return 0;
    }
    ((last_bytes.saturating_sub(*first_bytes)) as f64 / elapsed) as u64
}

/// Read the catalog declaration from `state_dir/vfs.toml`. A missing file
/// or absent `[catalog]` section means no catalog (`Ok(None)`); a section
/// that fails validation is an error, matching the daemon's behavior.
pub(crate) fn read_catalog_config(state_dir: &Path) -> Result<Option<CatalogConfig>> {
    let path = state_dir.join("vfs.toml");
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(color_eyre::eyre::eyre!(
                "cannot read {}: {error}",
                path.display()
            ));
        }
    };
    CatalogConfig::from_vfs_toml_optional(&source).map_err(|error| {
        color_eyre::eyre::eyre!("invalid catalog section in {}: {error}", path.display())
    })
}
