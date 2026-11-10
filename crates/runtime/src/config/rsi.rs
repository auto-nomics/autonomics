//! RSI plugin and environment (image) development configuration.

use container_runtime::ImageReference;

/// Fully resolved RSI plugin subsystem configuration.
///
/// The environment catalog and trusted GitHub publisher are startup
/// dependencies of `RsiInfra`, not optional runtime patches.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginRsiConfig {
    /// Approved digest-pinned environments available to plugin development.
    #[serde(default = "default_plugin_environments")]
    pub environments: plugin_rsi::EnvironmentCatalog,
    /// Trusted GitHub publication and update-PR configuration.
    #[serde(default)]
    pub publisher: plugin_rsi::GhPublisherConfig,
    /// Run background publication of locally active plugins.
    #[serde(default = "default_plugin_distillation_enabled")]
    pub distillation_enabled: bool,
    /// Background publication cadence in seconds.
    #[serde(default = "default_plugin_distillation_interval_secs")]
    pub distillation_interval_secs: u64,
    /// Environment (image) development subsystem configuration.
    #[serde(default)]
    pub environment_dev: EnvironmentDevConfig,
}

impl PluginRsiConfig {
    pub(crate) fn effective_distillation_interval_secs(&self) -> u64 {
        self.distillation_interval_secs.max(60)
    }
}

fn default_plugin_distillation_enabled() -> bool {
    true
}

fn default_plugin_distillation_interval_secs() -> u64 {
    5 * 60
}

/// Fully resolved environment (image) development configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EnvironmentDevConfig {
    /// Register the environment development tools and background distiller.
    #[serde(default = "default_environment_dev_enabled")]
    pub enabled: bool,
    /// Registry host environment images are pushed to.
    #[serde(default = "default_environment_dev_registry")]
    pub registry: String,
    /// Repository namespace below the registry host.
    #[serde(default = "default_environment_dev_namespace")]
    pub namespace: String,
    /// Run background publication of locally activated environments.
    #[serde(default = "default_environment_dev_distillation_enabled")]
    pub distillation_enabled: bool,
    /// Background environment publication cadence in seconds.
    #[serde(default = "default_environment_dev_distillation_interval_secs")]
    pub distillation_interval_secs: u64,
}

impl EnvironmentDevConfig {
    pub(crate) fn effective_distillation_interval_secs(&self) -> u64 {
        self.distillation_interval_secs.max(60)
    }
}

fn default_environment_dev_enabled() -> bool {
    true
}

fn default_environment_dev_registry() -> String {
    "ghcr.io".into()
}

fn default_environment_dev_namespace() -> String {
    "auto-nomics/environments".into()
}

fn default_environment_dev_distillation_enabled() -> bool {
    true
}

fn default_environment_dev_distillation_interval_secs() -> u64 {
    15 * 60
}

impl Default for EnvironmentDevConfig {
    fn default() -> Self {
        Self {
            enabled: default_environment_dev_enabled(),
            registry: default_environment_dev_registry(),
            namespace: default_environment_dev_namespace(),
            distillation_enabled: default_environment_dev_distillation_enabled(),
            distillation_interval_secs: default_environment_dev_distillation_interval_secs(),
        }
    }
}

impl Default for PluginRsiConfig {
    fn default() -> Self {
        Self {
            environments: default_plugin_environments(),
            publisher: Default::default(),
            distillation_enabled: default_plugin_distillation_enabled(),
            distillation_interval_secs: default_plugin_distillation_interval_secs(),
            environment_dev: Default::default(),
        }
    }
}

fn default_plugin_environments() -> plugin_rsi::EnvironmentCatalog {
    let mut environments = plugin_rsi::EnvironmentCatalog::default();

    // Every reference is an immutable manifest digest resolved from its
    // upstream registry. Only the base images are seeded: tool images
    // are published by this project under
    // `ghcr.io/auto-nomics/autonomics` and declared by the plugin
    // manifests, so seeding third-party tool images (BioContainers on
    // Quay, upstream Docker Hub builds) would only make `prepare` pull
    // gigabytes the daemon never runs.
    let mut add = |id: &str, reference: &str, interpreters: &[&str]| {
        let reference = ImageReference::parse(reference).expect("valid default image reference");
        environments.insert(
            id,
            plugin_rsi::Environment {
                reference: reference.to_string(),
                interpreters: interpreters
                    .iter()
                    .map(|interpreter| (*interpreter).to_string())
                    .collect(),
            },
        );
    };

    add(
        "alpine",
        "docker.io/library/alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507",
        &["sh"],
    );
    add(
        "debian",
        "docker.io/library/debian@sha256:c71b05eac0b20adb4cdcc9f7b052227efd7da381ad10bb92f972e8eae7c6cdc9",
        &["sh"],
    );
    add(
        "ubuntu",
        "docker.io/library/ubuntu@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55",
        &["sh"],
    );
    add(
        "python",
        "docker.io/library/python@sha256:5560e9ab8709f459489e5b8aa696eda8a07ef821e14bb122be62d91234bfa98b",
        &["sh", "python3"],
    );
    add(
        "rocker-verse",
        "docker.io/rocker/verse@sha256:408c531408bb3db00dacb2e98311d1a4671ba7f128688b3649630a6a480a239e",
        &["sh", "Rscript"],
    );
    add(
        "bioconductor",
        "docker.io/bioconductor/bioconductor@sha256:821dbf9ac119eac41f177531c7ca8fc7084c99c04eb218a1aa7f52cb63bad5d9",
        &["sh", "Rscript"],
    );

    environments
}
