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
    // upstream registry. BioContainers publishes current tool images on Quay.
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

    add(
        "samtools",
        "quay.io/biocontainers/samtools@sha256:a130447589651ed09252aa95a5e4f4132942cdb54d835d81a04a9a930d656561",
        &["sh", "samtools"],
    );
    add(
        "bcftools",
        "quay.io/biocontainers/bcftools@sha256:186c45de3059dbb1837006c27788284c89d17f8e693e7804624b6cf6ca5caca0",
        &["sh", "bcftools"],
    );
    add(
        "seqkit",
        "quay.io/biocontainers/seqkit@sha256:45fb535880be37dfed5be5517111fb8bfdd6234ef36e725b126a6131b1af2ef0",
        &["sh", "seqkit"],
    );
    add(
        "bwa",
        "quay.io/biocontainers/bwa@sha256:99a35e5ee4e9c329e8746c4689890b97a3ac5620cb36d374cba69ba52016e72a",
        &["sh", "bwa"],
    );
    add(
        "bowtie2",
        "quay.io/biocontainers/bowtie2@sha256:d568130f2f569f7ea2e2453e5f22e82d960d644b9a533b74f8ac737da6faf556",
        &["sh", "bowtie2"],
    );
    add(
        "hisat2",
        "quay.io/biocontainers/hisat2@sha256:dd9d15d12d46a3c49e7aa1cbf12cf49b2f80b54483fea1756b3fda4dde67e385",
        &["sh", "hisat2"],
    );
    add(
        "star-aligner",
        "quay.io/biocontainers/star@sha256:c43295cdbc28a7be7d88296775a29cc65b30296c996d8242ef90c031c8b9739b",
        &["sh", "STAR"],
    );
    add(
        "minimap2",
        "quay.io/biocontainers/minimap2@sha256:f7e4a9a17912eea542500988ee96013e37a887b3746673636f2773cc81052590",
        &["sh", "minimap2"],
    );
    add(
        "fastqc",
        "quay.io/biocontainers/fastqc@sha256:edb831dc3579dce16f49c7d8fd664eb74d0cfc0ed54ad4ea19ef409d254be934",
        &["sh", "fastqc"],
    );
    add(
        "fastp",
        "quay.io/biocontainers/fastp@sha256:7dcc9e128315636a3c471a82617ca7a103e823cc22d65beb28794d06c1920e1d",
        &["sh", "fastp"],
    );
    add(
        "multiqc",
        "quay.io/biocontainers/multiqc@sha256:ceace1ca5329886d92600c225d34a523020c163b1599403e44c8d895d2dfb2a2",
        &["sh", "multiqc"],
    );
    add(
        "trimmomatic",
        "quay.io/biocontainers/trimmomatic@sha256:2afd8a3c0bb068b6deb65597001d4d1b523a790c20a00dd026a5344c05bc7944",
        &["sh", "trimmomatic"],
    );
    add(
        "cutadapt",
        "quay.io/biocontainers/cutadapt@sha256:741216fb9a56cdac2a61e93c64f857bc5b3e1f9f2c123cf7ae2c6ea5c97702d8",
        &["sh", "cutadapt"],
    );
    add(
        "salmon",
        "quay.io/biocontainers/salmon@sha256:deee6c1353277c3fa9b579a79eff18f1588ad0a9f098c299d583c49d3613be9b",
        &["sh", "salmon"],
    );
    add(
        "kallisto",
        "quay.io/biocontainers/kallisto@sha256:6efe95704074d76fec299e4848185ccb1dc2b6805f48695b73717743dc22ca81",
        &["sh", "kallisto"],
    );
    add(
        "subread",
        "quay.io/biocontainers/subread@sha256:a0f7ea960dd0f55240b330b5c4515388a493e266a7b427d6a59a39164bc14865",
        &["sh", "featureCounts"],
    );
    add(
        "gatk4",
        "quay.io/biocontainers/gatk4@sha256:7311332e4d565d2356bbc0540cf582b505d1969a5f088a2c065302b13b10f5ee",
        &["sh", "gatk"],
    );
    add(
        "picard",
        "quay.io/biocontainers/picard@sha256:84923e37c5a43b0984fdb152a89f8ac952878fafc3e6076c534e5e5c82e278be",
        &["sh", "picard"],
    );
    add(
        "deepvariant",
        "quay.io/biocontainers/deepvariant@sha256:0e6b7249549d63b5cd864990422a562f3c5736e9041d9489bcad31da898ae24d",
        &["sh", "deepvariant"],
    );
    add(
        "ensembl-vep",
        "quay.io/biocontainers/ensembl-vep@sha256:fde86b9f4fc1c4b816c7c124d995a664d1711c7513f83920418afdd1fc2bbffa",
        &["sh", "vep"],
    );
    add(
        "snpeff",
        "quay.io/biocontainers/snpeff@sha256:832e86061ab3c1ef4339ec42f10fb98c4eaaecd498cd37d2bbe747e6bcc14de7",
        &["sh", "snpEff"],
    );
    add(
        "bedtools",
        "quay.io/biocontainers/bedtools@sha256:ffcd7aa5cd028e8156b8412cc4c88e4f7c4bfa44c4da24ac2737f82116940559",
        &["sh", "bedtools"],
    );
    add(
        "macs2",
        "quay.io/biocontainers/macs2@sha256:d1387523283200f0f09cc643266a6c9d1319e5b132b2b764015f75c04d61bff1",
        &["sh", "macs2"],
    );
    add(
        "deeptools",
        "quay.io/biocontainers/deeptools@sha256:59b35677305fca387aabecfa3f2fb9c79dab82c94dbe30d2e8ead3db99781a6d",
        &["sh", "python3"],
    );
    add(
        "bismark",
        "quay.io/biocontainers/bismark@sha256:86d39c42efab92a444949bc3a738e609b4787c1aad4285cfbceb2b54ed431e1b",
        &["sh", "bismark"],
    );
    add(
        "blast",
        "quay.io/biocontainers/blast@sha256:39337662941c833833009a4f024d82d7ffc8b6e68fdb5a099f2f31672aeed2ea",
        &["sh", "blastn", "blastp", "makeblastdb"],
    );
    add(
        "mafft",
        "quay.io/biocontainers/mafft@sha256:6e6f6f751df69b3aa0a0c243e8c01517c5e9e4393a491399cf05cb7b2b603184",
        &["sh", "mafft"],
    );
    add(
        "iqtree",
        "quay.io/biocontainers/iqtree@sha256:6e5792be5219238604dcf8b7b8ba6677c49b7764e410475dd00cbc691594fc63",
        &["sh", "iqtree3"],
    );
    add(
        "prodigal",
        "quay.io/biocontainers/prodigal@sha256:894e9100527f5c01c2f2c662723dacfe03d7d86f1e5cc5064d00b12e8494a6b1",
        &["sh", "prodigal"],
    );
    add(
        "kraken2",
        "quay.io/biocontainers/kraken2@sha256:db5d772d9df323cf0ddc644fb9793c0fca25b61f86469be42f1a9f5b903b9fdf",
        &["sh", "kraken2"],
    );

    add(
        "orthanc",
        "docker.io/orthancteam/orthanc@sha256:fd3e6fc7bf77b778f5cc66515bbe7bab70ce68e2a444849fe2ecf4eac75747a4",
        &["sh", "Orthanc"],
    );
    add(
        "ohif-viewer",
        "docker.io/ohif/viewer@sha256:b4bfecdc7cc670101cbfd01215fcd1f77b7cc5c867c5ed1cb10142f7a806ccd9",
        &["sh", "node"],
    );
    add(
        "monai",
        "docker.io/projectmonai/monai@sha256:996390500c689af285261b6616848b83c2ecf89291b4e891aaa6209e335a055e",
        &["sh", "python3"],
    );

    environments
}
