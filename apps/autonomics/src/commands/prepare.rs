//! `autonomics prepare` implementation — the explicit, network-bound
//! provisioning phase. The startup gate (`serve`) and `prepare
//! --check` only *check* readiness (local, offline-safe); this command
//! fills the gaps: plugin checkouts (git), panel data bundles, the
//! embedding model (atomic Hub download), and missing pinned images.
//!
//! Idempotent and re-runnable: every phase is a no-op when its state
//! already satisfies the readiness check.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use color_eyre::Result;
use i18n::t;
use plugin::factory::Plugin;

use crate::cli::PrepareArgs;
use crate::commands::panels::{load_installed_plugins, provision_missing_bundles};
use crate::diagnostics;

pub fn run_prepare(args: PrepareArgs) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    let config = gateway::RuntimeConfig::default();
    let result = runtime.block_on(async move {
        if args.check {
            check_only(&config).await
        } else {
            provision(&config).await
        }
    });
    // Bounded shutdown (mirrors `serve` / `panels`): background keep
    // alive connections must not pin the process after the summary.
    runtime.shutdown_timeout(std::time::Duration::from_secs(5));
    result
}

/// `prepare --check`: print the readiness report, exit 1 on fatal gaps.
async fn check_only(config: &gateway::RuntimeConfig) -> Result<()> {
    let report = diagnostics::check_readiness(config).await;
    report.print();
    if report.has_fatal_gaps() {
        return Err(color_eyre::eyre::eyre!("{}", t!("cli.prepare.checkNotReady")));
    }
    Ok(())
}

/// The full provisioning pass, in dependency order: plugins first
/// (their manifests feed image enumeration and panel bundles), then
/// panels, the model, images — and a final verification pass.
async fn provision(config: &gateway::RuntimeConfig) -> Result<()> {
    let plugins = sync_plugins(config)?;
    provision_panels(config).await?;
    install_model(config).await?;
    pull_missing_images(config, &plugins).await?;

    // Verify the end state with the exact checks the daemon gate runs.
    let report = diagnostics::check_readiness(config).await;
    report.print();
    if report.has_fatal_gaps() {
        return Err(color_eyre::eyre::eyre!("{}", t!("cli.prepare.provisioningIncomplete")));
    }
    println!("{}", t!("cli.prepare.complete"));
    Ok(())
}

/// Sync every declared plugin family to its pinned rev (git, network
/// when the rev is not local yet) and validate every manifest.
fn sync_plugins(config: &gateway::RuntimeConfig) -> Result<Vec<Plugin>> {
    let layout = plugin_rsi::PluginStateLayout::open(&config.state_dir);
    layout
        .ensure_v2_directories()
        .map_err(|error| color_eyre::eyre::eyre!("{}", t!("cli.diag.invalidPluginLayout", error = error)))?;
    let root = layout.runtime_root();
    let config_path = config.state_dir.join(plugin::sync::PLUGIN_CONFIG_FILE);
    if !config_path.is_file() {
        println!(
            "{}",
            t!("cli.diag.pluginsNotPresent", path = config_path.display())
        );
        return Ok(Vec::new());
    }

    println!(
        "{}",
        t!("cli.prepare.pluginsSyncing", path = config_path.display())
    );
    let report = plugin::sync::sync(&config_path, &root)
        .map_err(|error| color_eyre::eyre::eyre!("plugin sync failed: {error}"))?;
    for (name, outcome) in &report.outcomes {
        match outcome {
            plugin::sync::EntryOutcome::Installed => {
                println!("  + {}", t!("cli.prepare.pluginInstalled", name = name))
            }
            plugin::sync::EntryOutcome::Updated => {
                println!("  ~ {}", t!("cli.prepare.pluginUpdated", name = name))
            }
            plugin::sync::EntryOutcome::Unchanged => {}
        }
    }
    if report.outcomes.is_empty() {
        println!("  {}", t!("cli.prepare.noPluginSources"));
    }

    // Loading validates every manifest fail-closed.
    let plugins = load_installed_plugins(&config.state_dir)?;
    let kinds: usize = plugins.iter().map(|p| p.registered_kinds().len()).sum();
    println!(
        "plugins: {}",
        t!("cli.diag.pluginsSummary", count = plugins.len(), kinds = kinds)
    );
    Ok(plugins)
}

/// Fetch every declared panel data bundle into the local cache.
async fn provision_panels(config: &gateway::RuntimeConfig) -> Result<()> {
    let report = provision_missing_bundles(&config.state_dir).await?;
    let counts = report.counts();
    if counts.unavailable > 0 {
        let missing = report.missing_repos().join(", ");
        return Err(color_eyre::eyre::eyre!(
            "{}",
            if counts.unavailable == 1 {
                t!(
                    "cli.prepare.bundlesUnavailableOne",
                    count = counts.unavailable,
                    missing = missing
                )
            } else {
                t!(
                    "cli.prepare.bundlesUnavailableMany",
                    count = counts.unavailable,
                    missing = missing
                )
            }
        ));
    }
    Ok(())
}

/// Verify Ollama embedding configuration. No-op when the feature is
/// off — the daemon then runs on the corpus-fitted BM25 fallback.
async fn install_model(config: &gateway::RuntimeConfig) -> Result<()> {
    if !config.embedding.enabled {
        println!("{}", t!("cli.prepare.modelSkipped"));
        return Ok(());
    }
    println!(
        "{}",
        t!(
            "cli.prepare.modelConfigured",
            model = config.embedding.model,
            url = config.embedding.base_url
        )
    );
    Ok(())
}

/// Default concurrent image pre-pull width, chosen by storage driver:
/// overlay commits layers with cheap hardlink/mount work, so the store
/// lock rotates quickly and parallel pulls pay off; vfs commits are
/// full-copy extractions that hold the store lock for the whole layer,
/// so wide concurrency only queues pull processes behind one writer —
/// and each Podman pull already opens several parallel layer downloads,
/// so two concurrent pulls multiply the load on a flaky link.
/// `AUTONOMICS_IMAGE_PULL_CONCURRENCY` overrides (clamped to 1..=16;
/// 1 restores a fully sequential pull).
fn pull_concurrency(storage_driver: Option<&str>) -> usize {
    let default = match storage_driver {
        Some("overlay") => 4,
        // vfs and any unrecognised/legacy setting stay conservative.
        _ => 1,
    };
    std::env::var("AUTONOMICS_IMAGE_PULL_CONCURRENCY")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, 16))
        .unwrap_or(default)
}

/// Short human label for a digest-pinned reference — the final path
/// segment of the name part ("samtools" for
/// "quay.io/biocontainers/samtools@sha256:…").
fn short_label(reference: &str) -> &str {
    let name = reference.split('@').next().unwrap_or(reference);
    name.rsplit('/').next().unwrap_or(name)
}

/// Pre-pull every declared image missing from the local store, into the
/// exact private backend the daemon creates containers in. Pulls run
/// with bounded concurrency; podman's stderr progress is relayed per
/// image and a summary line prints every 15 seconds.
async fn pull_missing_images(config: &gateway::RuntimeConfig, plugins: &[Plugin]) -> Result<()> {
    let declared: BTreeSet<String> = diagnostics::enumerate_images(config, plugins).await?;
    let infra =
        container_runtime::ContainerExecutionInfra::for_operator_state_dir(&config.state_dir)
            .map_err(|error| {
                color_eyre::eyre::eyre!(
                    "{}",
                    t!("cli.diag.imagesConfigFailed", error = error)
                )
            })?;
    let local = container_runtime::images::local_images_with_config(&infra.config)
        .await
        .map_err(|error| color_eyre::eyre::eyre!("cannot list local images: {error}"))?;
    let missing: Vec<String> = declared
        .iter()
        .filter(|reference| !local.contains(reference))
        .cloned()
        .collect();
    if missing.is_empty() {
        println!(
            "{}",
            t!("cli.prepare.imagesAllPresent", count = declared.len())
        );
        return Ok(());
    }
    let concurrency = pull_concurrency(infra.config.storage_driver.as_deref());
    println!(
        "{}",
        t!(
            "cli.prepare.imagesPulling",
            count = declared.len(),
            missing = missing.len(),
            concurrency = concurrency
        )
    );
    let started = std::time::Instant::now();
    let permits = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let done = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let failures: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let mut set = tokio::task::JoinSet::new();
    for reference in missing {
        let permit = permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| color_eyre::eyre::eyre!("pull gate closed: {error}"))?;
        let pull_config = infra.config.clone();
        let done = done.clone();
        let failed = failed.clone();
        let failures = failures.clone();
        set.spawn(async move {
            let _permit = permit;
            let label = short_label(&reference).to_string();
            let pull_started = std::time::Instant::now();
            println!("  → {}", t!("cli.prepare.pullingImage", label = label));
            let result = container_runtime::images::pull_with_config_progress(
                &pull_config,
                &reference,
                |line| println!("    {label} │ {line}"),
            )
            .await;
            match result {
                Ok(()) => {
                    done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    println!(
                        "  {}",
                        t!(
                            "cli.prepare.pullOk",
                            label = label,
                            secs = format!("{:.1}s", pull_started.elapsed().as_secs_f32())
                        )
                    );
                }
                Err(error) => {
                    failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    println!(
                        "  {}",
                        t!("cli.prepare.pullFailed", label = label, error = error)
                    );
                    failures.lock().unwrap().push(reference);
                }
            }
        });
    }
    // Periodic overall progress while the workers pull.
    let ticker = {
        let done = done.clone();
        let failed = failed.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                println!(
                    "{}",
                    t!(
                        "cli.prepare.pullProgress",
                        done = done.load(std::sync::atomic::Ordering::Relaxed),
                        total = done.load(std::sync::atomic::Ordering::Relaxed)
                            + failed.load(std::sync::atomic::Ordering::Relaxed),
                        failed = failed.load(std::sync::atomic::Ordering::Relaxed),
                        secs = format!("{:.0}", started.elapsed().as_secs_f32()),
                    )
                );
            }
        })
    };
    while let Some(outcome) = set.join_next().await {
        if let Err(error) = outcome {
            eprintln!("{}", t!("cli.prepare.pullWorkerPanicked", error = error));
        }
    }
    ticker.abort();
    let failed_refs = failures.lock().unwrap().clone();
    if !failed_refs.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "{}",
            t!(
                "cli.prepare.imagePullFailedError",
                count = failed_refs.len(),
                refs = failed_refs.join(", ")
            )
        ));
    }
    println!(
        "{}",
        t!(
            "cli.prepare.imagesPulled",
            count = done.load(std::sync::atomic::Ordering::Relaxed),
            secs = format!("{:.1}s", started.elapsed().as_secs_f32())
        )
    );
    Ok(())
}
