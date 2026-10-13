//! Offline readiness checks shared by `prepare`, `prepare --check`,
//! the bare-`autonomics` guide, and the serve startup gate.
//!
//! Zero network by design: plugin rev-presence via local git
//! (`container_plugin::sync::check`), the model via its completion
//! marker (`text_embedding::verify_install`), images via one
//! `podman images` call. The provisioning that fills the gaps these
//! checks find is `prepare`'s job — and the daemon's internal
//! `materialize_registry` sync stays as an idempotent, network-free
//! no-op backstop once the gate has passed.

use std::collections::BTreeSet;
use std::path::PathBuf;

use container_plugin::factory::Plugin;

use crate::commands::panels::load_installed_plugins;

/// The shape of one finding: what is absent or wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    Missing,
    Inconsistent,
}

/// One readiness gap: a subject plus why it is not ready.
#[derive(Debug, Clone)]
pub struct Gap {
    pub subject: String,
    pub kind: GapKind,
    pub detail: String,
}

impl Gap {
    fn missing(subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            kind: GapKind::Missing,
            detail: detail.into(),
        }
    }

    fn inconsistent(subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            kind: GapKind::Inconsistent,
            detail: detail.into(),
        }
    }
}

/// The outcome of one readiness category.
#[derive(Debug, Clone)]
pub enum Category {
    /// Ready, with a human-readable summary.
    Ok(String),
    /// Nothing to check (no plugins.toml / embedding disabled) and why.
    Skipped(String),
    /// One or more findings — fatal for plugins, model (when enabled),
    /// and images; advisory for panels.
    Gap(Vec<Gap>),
    /// The check itself errored (unreadable config, git or podman
    /// missing). Treated like a gap by [`ReadinessReport::has_fatal_gaps`].
    Failed(String),
}

impl Category {
    fn is_ready(&self) -> bool {
        matches!(self, Category::Ok(_) | Category::Skipped(_))
    }
}

/// The full readiness picture: plugins, model, and images are fatal;
/// panels is advisory (a missing bundle only fails the nodes needing
/// it at build time — the deliberate tolerance `panel_bundle_preflight`
/// established).
#[derive(Debug, Clone)]
pub struct ReadinessReport {
    pub plugins: Category,
    pub model: Category,
    pub images: Category,
    pub panels: Category,
}

impl ReadinessReport {
    /// Whether any fatal category blocks daemon startup.
    pub fn has_fatal_gaps(&self) -> bool {
        [
            self.plugins.is_ready(),
            self.model.is_ready(),
            self.images.is_ready(),
        ]
        .iter()
        .any(|ready| !*ready)
    }

    /// Shared human rendering: one line per category, `  ! …` lines
    /// for findings, the prepare hint on every fatal one.
    pub fn print(&self) {
        print_category("plugins", &self.plugins, true);
        print_category("model", &self.model, true);
        print_category("images", &self.images, true);
        print_category("panels", &self.panels, false);
    }
}

fn print_category(name: &str, category: &Category, fatal: bool) {
    match category {
        Category::Ok(summary) => println!("{name:<8} ok   {summary}"),
        Category::Skipped(why) => println!("{name:<8} skip {why}"),
        Category::Failed(error) => {
            println!("{name:<8} FAIL {error}");
            if fatal {
                println!("         run `autonomics prepare`");
            }
        }
        Category::Gap(gaps) => {
            let hint = if fatal {
                " — run `autonomics prepare`"
            } else {
                " — run `autonomics prepare` to fetch the rest (nodes needing them fail at build time until then)"
            };
            println!("{name:<8} {} gap{}", gaps.len(), hint);
            for gap in gaps {
                let kind = match gap.kind {
                    GapKind::Missing => "missing",
                    GapKind::Inconsistent => "inconsistent",
                };
                println!("         ! {} [{kind}] {}", gap.subject, gap.detail);
            }
        }
    }
}

/// Run every readiness category against `config`. Zero network.
pub async fn check_readiness(config: &gateway::RuntimeConfig) -> ReadinessReport {
    let plugins = check_plugins(config).await;
    // The loaded families feed the images and panels categories; when
    // the plugin check did not pass there is nothing trustworthy to
    // enumerate.
    let loaded = match &plugins {
        Category::Ok(_) => load_installed_plugins(&config.state_dir).ok(),
        _ => None,
    };

    let model = check_model(config).await;
    let images = check_images(config, loaded.as_deref()).await;
    let panels = check_panels(loaded.as_deref());
    ReadinessReport {
        plugins,
        model,
        images,
        panels,
    }
}

/// Plugins: layout usable, every declared source installed at its pin
/// (offline), every manifest valid.
async fn check_plugins(config: &gateway::RuntimeConfig) -> Category {
    let layout = plugin_rsi::PluginStateLayout::open(&config.state_dir);
    if let Err(error) = layout.ensure_v2_directories() {
        return Category::Failed(format!("invalid plugin layout: {error}"));
    }
    let root = layout.runtime_root();
    let config_path = config
        .state_dir
        .join(container_plugin::sync::PLUGIN_CONFIG_FILE);
    if !config_path.is_file() {
        return Category::Skipped(format!(
            "{} not present — built-in nodes only",
            config_path.display()
        ));
    }

    let report = match container_plugin::sync::check(&config_path, &root) {
        Ok(report) => report,
        Err(error) => return Category::Failed(error.to_string()),
    };
    let gaps: Vec<Gap> = report
        .entries
        .into_iter()
        .filter(|entry| entry.outcome != container_plugin::sync::CheckOutcome::Ok)
        .map(|entry| {
            let (kind, label) = match entry.outcome {
                container_plugin::sync::CheckOutcome::Ok => unreachable!(),
                container_plugin::sync::CheckOutcome::Missing => (GapKind::Missing, "missing"),
                container_plugin::sync::CheckOutcome::Inconsistent => {
                    (GapKind::Inconsistent, "inconsistent")
                }
            };
            Gap {
                subject: format!("plugin '{}'", entry.name),
                kind,
                detail: if entry.detail.is_empty() {
                    label.to_string()
                } else {
                    entry.detail
                },
            }
        })
        .collect();
    if !gaps.is_empty() {
        return Category::Gap(gaps);
    }

    // Manifest validity is fail-closed: a declared-but-broken family
    // aborts startup naming the offender.
    match load_installed_plugins(&config.state_dir) {
        Ok(plugins) => {
            let kinds: usize = plugins.iter().map(|p| p.registered_kinds().len()).sum();
            let summary = format!(
                "{} famil{} verified, {kinds} node kinds",
                plugins.len(),
                if plugins.len() == 1 { "y" } else { "ies" },
            );
            tracing::info!(families = plugins.len(), kinds, "plugin readiness verified");
            Category::Ok(summary)
        }
        Err(error) => Category::Failed(error.to_string()),
    }
}

/// The embedding model: marker + file-set verification against the
/// configured (or default) directory.
async fn check_model(config: &gateway::RuntimeConfig) -> Category {
    if !config.enable_local_embedding {
        return Category::Skipped(
            "local embedding disabled — BM25 over the corpus serves the hub".into(),
        );
    }
    let dir: PathBuf = match config.embedding_model_dir.clone() {
        Some(dir) => dir,
        None => match text_embedding::default_model_dir() {
            Ok(dir) => dir,
            Err(error) => return Category::Failed(error.to_string()),
        },
    };
    match text_embedding::verify_install(&dir) {
        text_embedding::ModelVerification::Verified => {
            Category::Ok(format!("verified at {}", dir.display()))
        }
        text_embedding::ModelVerification::CompleteWithoutMarker => Category::Ok(format!(
            "complete at {} (no completion marker — prepare will stamp one)",
            dir.display()
        )),
        text_embedding::ModelVerification::Missing(files) => Category::Gap(
            files
                .into_iter()
                .map(|file| Gap::missing(format!("model file '{file}'"), "not installed"))
                .collect(),
        ),
        text_embedding::ModelVerification::Inconsistent(problems) => Category::Gap(
            problems
                .into_iter()
                .map(|problem| Gap::inconsistent("model", problem))
                .collect(),
        ),
    }
}

/// Enumerate every digest-pinned image reference the daemon can need
/// at startup: the environment catalog, every loaded plugin family's
/// manifest image, and the built-in script pins. Per-DAG
/// `container_command` images are runtime data and excluded.
pub(crate) async fn enumerate_images(
    config: &gateway::RuntimeConfig,
    plugins: &[Plugin],
) -> color_eyre::Result<BTreeSet<String>> {
    let mut declared: BTreeSet<String> = BTreeSet::new();

    // Environment catalog: opening the registry seeds the file on
    // first launch (the same side effect the daemon performs in
    // SharedInfra::open), so the enumeration sees exactly what the
    // daemon will see.
    let registry_path = config.state_dir.join("plugin-environments.toml");
    let registry = plugin_rsi::EnvironmentRegistry::open(
        &registry_path,
        config.plugin_rsi.environments.clone(),
    )
    .map_err(|error| color_eyre::eyre::eyre!("environment registry: {error}"))?;
    for environment in registry.snapshot().list().values() {
        declared.insert(environment.reference.clone());
    }

    for plugin in plugins {
        declared.insert(plugin.image_reference().to_string());
    }
    declared.extend(nodes_io::builtin_script_images());
    Ok(declared)
}

/// Images: every enumerable digest-pinned reference (environment
/// catalog + plugin manifests + built-in script pins) present in the
/// local podman store.
async fn check_images(config: &gateway::RuntimeConfig, plugins: Option<&[Plugin]>) -> Category {
    let Some(plugins) = plugins else {
        return Category::Failed(
            "cannot enumerate plugin images — the plugin check did not pass".into(),
        );
    };
    let declared = match enumerate_images(config, plugins).await {
        Ok(declared) => declared,
        Err(error) => return Category::Failed(error.to_string()),
    };

    let program = container_runtime::PodmanConfig::from_env().program;
    let local = match container_runtime::images::local_digests(&program).await {
        Ok(local) => local,
        Err(error) => return Category::Failed(error.to_string()),
    };
    let missing: Vec<Gap> = declared
        .iter()
        .filter(|reference| !local.contains(*reference))
        .map(|reference| Gap::missing(format!("image '{reference}'"), "not in the local store"))
        .collect();
    if missing.is_empty() {
        Category::Ok(format!(
            "{} pinned images present in the local store",
            declared.len()
        ))
    } else {
        Category::Gap(missing)
    }
}

/// Panel bundles (advisory): local-only presence check against the
/// panel cache, exactly the tolerance the old preflight established.
fn check_panels(plugins: Option<&[Plugin]>) -> Category {
    let Some(plugins) = plugins else {
        return Category::Skipped("no loadable plugin families".into());
    };
    let bundle_ids = container_plugin::bundles::collect_panel_bundles(plugins);
    if bundle_ids.is_empty() {
        return Category::Ok("no data bundles declared".into());
    }
    let local = match data_catalog::LocalCatalog::open(data_catalog::default_panel_cache_root()) {
        Ok(local) => local,
        Err(error) => return Category::Failed(format!("panel cache: {error}")),
    };
    let report = container_plugin::bundles::check_panel_bundles(&bundle_ids, &local);
    let counts = report.counts();
    if counts.unavailable == 0 {
        Category::Ok(format!(
            "{}/{} data bundles cached",
            counts.cached + counts.installed,
            bundle_ids.len()
        ))
    } else {
        Category::Gap(
            report
                .missing_repos()
                .into_iter()
                .map(|repo| Gap::missing(format!("panel bundle '{repo}'"), "not cached"))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Category {
        Category::Ok("fine".into())
    }

    fn gap() -> Category {
        Category::Gap(vec![Gap::missing("subject", "detail")])
    }

    #[test]
    fn fatal_categories_decide_has_fatal_gaps() {
        let report = |plugins, model, images| ReadinessReport {
            plugins,
            model,
            images,
            panels: Category::Ok("cached".into()),
        };

        // All ready.
        assert!(!report(ok(), ok(), ok()).has_fatal_gaps());
        // Skipped is ready (embedding disabled / no plugins.toml).
        assert!(
            !report(
                Category::Skipped("no plugins.toml".into()),
                Category::Skipped("disabled".into()),
                ok()
            )
            .has_fatal_gaps()
        );
        // Each fatal category, gapped or failed, blocks.
        assert!(report(gap(), ok(), ok()).has_fatal_gaps());
        assert!(report(ok(), gap(), ok()).has_fatal_gaps());
        assert!(report(ok(), ok(), gap()).has_fatal_gaps());
        assert!(report(Category::Failed("git missing".into()), ok(), ok()).has_fatal_gaps());
        assert!(report(ok(), Category::Failed("podman missing".into()), ok()).has_fatal_gaps());
    }

    #[test]
    fn panel_gaps_are_advisory() {
        let report = ReadinessReport {
            plugins: ok(),
            model: ok(),
            images: ok(),
            panels: Category::Gap(vec![Gap::missing("panel bundle 'x'", "not cached")]),
        };
        assert!(
            !report.has_fatal_gaps(),
            "a missing panel bundle never blocks daemon startup"
        );
    }
}
