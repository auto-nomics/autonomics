//! `autonomics prepare` implementation — the explicit, network-bound
//! provisioning phase. The startup gate (`serve`) and `prepare
//! --check` only *check* readiness (local, offline-safe); this command
//! fills the gaps: plugin checkouts (git), panel data bundles, the
//! embedding model (atomic Hub download), and missing pinned images.
//!
//! Idempotent and re-runnable: every phase is a no-op when its state
//! already satisfies the readiness check.

use std::collections::BTreeSet;

use color_eyre::Result;
use container_plugin::factory::Plugin;

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
        return Err(color_eyre::eyre::eyre!(
            "not ready — run `autonomics prepare` to provision the gaps above"
        ));
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
        return Err(color_eyre::eyre::eyre!(
            "provisioning incomplete — the gaps above remain after `prepare`; \\
             inspect the errors above and re-run"
        ));
    }
    println!("prepare complete — daemon ready to start (`autonomics serve`)");
    Ok(())
}

/// Sync every declared plugin family to its pinned rev (git, network
/// when the rev is not local yet) and validate every manifest.
fn sync_plugins(config: &gateway::RuntimeConfig) -> Result<Vec<Plugin>> {
    let layout = plugin_rsi::PluginStateLayout::open(&config.state_dir);
    layout
        .ensure_v2_directories()
        .map_err(|error| color_eyre::eyre::eyre!("invalid plugin layout: {error}"))?;
    let root = layout.runtime_root();
    let config_path = config
        .state_dir
        .join(container_plugin::sync::PLUGIN_CONFIG_FILE);
    if !config_path.is_file() {
        println!(
            "plugins: {} not present — built-in nodes only",
            config_path.display()
        );
        return Ok(Vec::new());
    }

    println!("plugins: syncing from {}", config_path.display());
    let report = container_plugin::sync::sync(&config_path, &root)
        .map_err(|error| color_eyre::eyre::eyre!("plugin sync failed: {error}"))?;
    for (name, outcome) in &report.outcomes {
        match outcome {
            container_plugin::sync::EntryOutcome::Installed => {
                println!("  + {name} installed")
            }
            container_plugin::sync::EntryOutcome::Updated => println!("  ~ {name} updated"),
            container_plugin::sync::EntryOutcome::Unchanged => {}
        }
    }
    if report.outcomes.is_empty() {
        println!("  (no plugin sources declared)");
    }

    // Loading validates every manifest fail-closed.
    let plugins = load_installed_plugins(&config.state_dir)?;
    let kinds: usize = plugins.iter().map(|p| p.registered_kinds().len()).sum();
    println!(
        "plugins: {} famil{} verified, {kinds} node kinds",
        plugins.len(),
        if plugins.len() == 1 { "y" } else { "ies" }
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
            "provisioning incomplete: {} bundle{} unavailable ({missing})",
            counts.unavailable,
            if counts.unavailable == 1 { "" } else { "s" },
        ));
    }
    Ok(())
}

/// Verify Ollama embedding configuration. No-op when the feature is
/// off — the daemon then runs on the corpus-fitted BM25 fallback.
async fn install_model(config: &gateway::RuntimeConfig) -> Result<()> {
    if !config.embedding.enabled {
        println!("model: skipped — Ollama embedding disabled");
        return Ok(());
    }
    println!(
        "model: Ollama embedding configured — {} @ {}",
        config.embedding.model, config.embedding.base_url
    );
    Ok(())
}

/// Pre-pull every declared image missing from the local store, one at
/// a time (concurrent storage mutation is what wedges podman's store).
async fn pull_missing_images(config: &gateway::RuntimeConfig, plugins: &[Plugin]) -> Result<()> {
    let declared: BTreeSet<String> = diagnostics::enumerate_images(config, plugins).await?;
    let program = container_runtime::PodmanConfig::from_env().program;
    let local = container_runtime::images::local_digests(&program)
        .await
        .map_err(|error| color_eyre::eyre::eyre!("cannot list local images: {error}"))?;
    let missing: Vec<&String> = declared
        .iter()
        .filter(|reference| !local.contains(*reference))
        .collect();
    if missing.is_empty() {
        println!("images: {} declared, all present", declared.len());
        return Ok(());
    }

    println!(
        "images: {} declared, {} missing",
        declared.len(),
        missing.len()
    );
    for reference in &missing {
        println!("  pulling {reference}");
        container_runtime::images::pull(&program, reference)
            .await
            .map_err(|error| {
                color_eyre::eyre::eyre!("image pull failed for {reference}: {error}")
            })?;
    }
    println!("images: pulled {} missing images", missing.len());
    Ok(())
}
