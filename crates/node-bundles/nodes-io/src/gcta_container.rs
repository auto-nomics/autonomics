//! Containerized GCTA summary-statistics nodes.
//!
//! The factories own the verified compatibility contract: official GCTA
//! 1.95.3, the 1000G EUR Phase3 PLINK LD reference, and the official GCTA
//! hg19 gene list. Callers select an analysis and provide its summary File.

use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{
    DataBundleBinding, NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs,
    value::PortType,
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
    ContainerPanelBundleSpec,
};
use crate::image_registry::registry_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const GCTA_COJO_SELECT_CONTAINER_KIND: &str = "gcta_cojo_select_container";
pub const GCTA_SBLUP_CONTAINER_KIND: &str = "gcta_sblup_container";
pub const GCTA_FASTBAT_CONTAINER_KIND: &str = "gcta_fastbat_container";
pub const GCTA_ACAT_CONTAINER_KIND: &str = "gcta_acat_container";
pub const GCTA_ORIGINAL_IMAGE_REPOSITORY: &str = "gcta";
pub const GCTA_ORIGINAL_IMAGE_DIGEST: &str =
    "sha256:4cbf8c91376f7b314eebf1dfa02ad44028575991cd3d4c4e81b5324942bec20b";
pub const GCTA_REF_BINARY_PANEL: &str = "wjixiang/catalog-plink-ref-1000g-eur-binary";
pub const GCTA_GENE_LIST_PANEL: &str = "wjixiang/catalog-gcta-gene-list-hg19";

const DEFAULT_MAF: f64 = 0.01;
const DEFAULT_COJO_P: f64 = 5e-8;
const DEFAULT_COJO_WIND_KB: u32 = 10_000;
const DEFAULT_SBLUP_WIND_KB: u32 = 1_000;
const DEFAULT_DIFF_FREQ: f64 = 0.2;
const DEFAULT_COJO_COLLINEAR: f64 = 0.9;
const DEFAULT_FASTBAT_WIND_KB: u32 = 50;
const DEFAULT_FASTBAT_LD_CUTOFF: f64 = 0.9;
const DEFAULT_ACAT_MAX_MAF: f64 = 0.01;
const DEFAULT_ACAT_MIN_MAC: u32 = 20;
const DEFAULT_THREAD_NUM: u32 = 1;
const DEFAULT_TIMEOUT_SECS: u64 = 3600;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GctaCojoSelectContainerSpec {
    pub chr: u8,
    #[serde(default = "default_maf")]
    pub maf: f64,
    #[serde(default = "default_cojo_p")]
    pub cojo_p: f64,
    #[serde(default = "default_cojo_wind_kb")]
    pub cojo_wind_kb: u32,
    #[serde(default = "default_cojo_collinear")]
    pub cojo_collinear: f64,
    #[serde(default = "default_diff_freq")]
    pub diff_freq: f64,
    #[serde(default = "default_thread_num")]
    pub thread_num: u32,
    #[serde(default = "default_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GctaSblupContainerSpec {
    pub chr: u8,
    pub lambda: f64,
    #[serde(default = "default_maf")]
    pub maf: f64,
    #[serde(default = "default_sblup_wind_kb")]
    pub cojo_wind_kb: u32,
    #[serde(default = "default_diff_freq")]
    pub diff_freq: f64,
    #[serde(default = "default_thread_num")]
    pub thread_num: u32,
    #[serde(default = "default_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GctaFastbatContainerSpec {
    pub chr: u8,
    #[serde(default = "default_maf")]
    pub maf: f64,
    #[serde(default = "default_fastbat_wind_kb")]
    pub gene_flank_kb: u32,
    #[serde(default = "default_fastbat_ld_cutoff")]
    pub ld_cutoff: f64,
    #[serde(default = "default_diff_freq")]
    pub diff_freq: f64,
    #[serde(default = "default_thread_num")]
    pub thread_num: u32,
    #[serde(default = "default_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GctaAcatContainerSpec {
    #[serde(default = "default_acat_max_maf")]
    pub max_maf: f64,
    #[serde(default = "default_acat_min_mac")]
    pub min_mac: u32,
    #[serde(default)]
    pub gene_flank_kb: u32,
    #[serde(default = "default_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_maf() -> f64 {
    DEFAULT_MAF
}

fn default_cojo_p() -> f64 {
    DEFAULT_COJO_P
}

fn default_cojo_wind_kb() -> u32 {
    DEFAULT_COJO_WIND_KB
}

fn default_sblup_wind_kb() -> u32 {
    DEFAULT_SBLUP_WIND_KB
}

fn default_cojo_collinear() -> f64 {
    DEFAULT_COJO_COLLINEAR
}

fn default_fastbat_wind_kb() -> u32 {
    DEFAULT_FASTBAT_WIND_KB
}

fn default_fastbat_ld_cutoff() -> f64 {
    DEFAULT_FASTBAT_LD_CUTOFF
}

fn default_acat_max_maf() -> f64 {
    DEFAULT_ACAT_MAX_MAF
}

fn default_acat_min_mac() -> u32 {
    DEFAULT_ACAT_MIN_MAC
}

fn default_diff_freq() -> f64 {
    DEFAULT_DIFF_FREQ
}

fn default_thread_num() -> u32 {
    DEFAULT_THREAD_NUM
}

fn default_prefix() -> String {
    "/artifacts/gcta_container".into()
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn validate_common(
    chr: Option<u8>,
    maf: f64,
    diff_freq: Option<f64>,
    thread_num: u32,
    prefix: &str,
    timeout_secs: u64,
) -> Result<(), String> {
    if let Some(chr) = chr
        && !(1..=22).contains(&chr)
    {
        return Err("chr must lie in 1..=22".into());
    }
    if !maf.is_finite() || maf <= 0.0 || maf > 0.5 {
        return Err("maf must lie in (0, 0.5]".into());
    }
    if let Some(value) = diff_freq
        && (!value.is_finite() || value <= 0.0 || value > 1.0)
    {
        return Err("diff_freq must lie in (0, 1]".into());
    }
    if thread_num == 0 || thread_num > 1024 {
        return Err("thread_num must lie in 1..=1024".into());
    }
    if timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if !prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

pub fn validate_cojo_select(spec: &GctaCojoSelectContainerSpec) -> Result<(), String> {
    validate_common(
        Some(spec.chr),
        spec.maf,
        Some(spec.diff_freq),
        spec.thread_num,
        &spec.artifact_prefix,
        spec.timeout_secs,
    )?;
    if !spec.cojo_p.is_finite() || spec.cojo_p <= 0.0 || spec.cojo_p > 1.0 {
        return Err("cojo_p must lie in (0, 1]".into());
    }
    if spec.cojo_wind_kb == 0 {
        return Err("cojo_wind_kb must be greater than zero".into());
    }
    if !spec.cojo_collinear.is_finite() || spec.cojo_collinear <= 0.0 || spec.cojo_collinear >= 1.0
    {
        return Err("cojo_collinear must lie in (0, 1)".into());
    }
    Ok(())
}

pub fn validate_sblup(spec: &GctaSblupContainerSpec) -> Result<(), String> {
    validate_common(
        Some(spec.chr),
        spec.maf,
        Some(spec.diff_freq),
        spec.thread_num,
        &spec.artifact_prefix,
        spec.timeout_secs,
    )?;
    if !spec.lambda.is_finite() || spec.lambda <= 0.0 {
        return Err("lambda must be finite and greater than zero".into());
    }
    if spec.cojo_wind_kb == 0 {
        return Err("cojo_wind_kb must be greater than zero".into());
    }
    Ok(())
}

pub fn validate_fastbat(spec: &GctaFastbatContainerSpec) -> Result<(), String> {
    validate_common(
        Some(spec.chr),
        spec.maf,
        Some(spec.diff_freq),
        spec.thread_num,
        &spec.artifact_prefix,
        spec.timeout_secs,
    )?;
    if spec.gene_flank_kb == 0 {
        return Err("gene_flank_kb must be greater than zero".into());
    }
    if !spec.ld_cutoff.is_finite() || spec.ld_cutoff <= 0.0 || spec.ld_cutoff > 1.0 {
        return Err("ld_cutoff must lie in (0, 1]".into());
    }
    Ok(())
}

pub fn validate_acat(spec: &GctaAcatContainerSpec) -> Result<(), String> {
    validate_common(None, 0.5, None, 1, &spec.artifact_prefix, spec.timeout_secs)?;
    if !spec.max_maf.is_finite() || spec.max_maf <= 0.0 || spec.max_maf > 0.5 {
        return Err("max_maf must lie in (0, 0.5]".into());
    }
    if spec.min_mac == 0 {
        return Err("min_mac must be greater than zero".into());
    }
    Ok(())
}

fn command_spec(
    script: String,
    prefix: String,
    timeout_secs: u64,
    outputs: Vec<ContainerCommandOutputSpec>,
    panels: Vec<ContainerPanelBundleSpec>,
) -> Result<ContainerCommandSpec, String> {
    Ok(ContainerCommandSpec {
        image: registry_image(GCTA_ORIGINAL_IMAGE_REPOSITORY, GCTA_ORIGINAL_IMAGE_DIGEST)?,
        command: vec!["sh".into(), "-c".into()],
        script: Some(script),
        files: Default::default(),
        env: Default::default(),
        outputs,
        workdir: None,
        artifact_prefix: prefix,
        timeout_secs,
        panels: Vec::new(),
        panel_bundles: panels,
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
        cpus: None,
        memory: None,
        pids_limit: None,
        shm_size: None,
        gpus: None,
        user: None,
    })
}

pub fn cojo_select_container_spec(
    spec: &GctaCojoSelectContainerSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_cojo_select(spec)?;
    let script = format!(
        "set -eu\n\
         gcta64 \\\n\
         \x20\x20\x20 --bfile /panels/plink_ref/1000G.EUR.QC.{chr} \\\n\
         \x20\x20\x20 --chr {chr} \\\n\
         \x20\x20\x20 --maf {maf} \\\n\
         \x20\x20\x20 --cojo-file \"${{AUTONOMICS_INPUT0}}\" \\\n\
         \x20\x20\x20 --cojo-slct \\\n\
         \x20\x20\x20 --cojo-p {cojo_p:e} \\\n\
         \x20\x20\x20 --cojo-wind {cojo_wind_kb} \\\n\
         \x20\x20\x20 --cojo-collinear {cojo_collinear} \\\n\
         \x20\x20\x20 --diff-freq {diff_freq} \\\n\
         \x20\x20\x20 --thread-num {thread_num} \\\n\
         \x20\x20\x20 --out /work/gcta_cojo \\\n\
         \x20\x20\x20> \"${{AUTONOMICS_OUTPUT3}}\" 2>&1\n\
         test -s \"${{AUTONOMICS_OUTPUT0}}\"\n\
         test -s \"${{AUTONOMICS_OUTPUT1}}\"\n",
        chr = spec.chr,
        maf = spec.maf,
        cojo_p = spec.cojo_p,
        cojo_wind_kb = spec.cojo_wind_kb,
        cojo_collinear = spec.cojo_collinear,
        diff_freq = spec.diff_freq,
        thread_num = spec.thread_num,
    );
    command_spec(
        script,
        spec.artifact_prefix.clone(),
        spec.timeout_secs,
        vec![
            ContainerCommandOutputSpec {
                path: "gcta_cojo.jma.cojo".into(),
                format: Some("gcta_cojo_jma".into()),
            },
            ContainerCommandOutputSpec {
                path: "gcta_cojo.ldr.cojo".into(),
                format: Some("gcta_cojo_ldr".into()),
            },
            ContainerCommandOutputSpec {
                path: "gcta_cojo.cma.cojo".into(),
                format: Some("gcta_cojo_cma".into()),
            },
            ContainerCommandOutputSpec {
                path: "gcta_cojo.log".into(),
                format: Some("gcta_log".into()),
            },
        ],
        vec![ContainerPanelBundleSpec {
            panel_id: GCTA_REF_BINARY_PANEL.into(),
            mount_path: "/panels/plink_ref".into(),
        }],
    )
}

pub fn sblup_container_spec(spec: &GctaSblupContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate_sblup(spec)?;
    let script = format!(
        "set -eu\n\
         gcta64 \\\n\
         \x20\x20\x20 --bfile /panels/plink_ref/1000G.EUR.QC.{chr} \\\n\
         \x20\x20\x20 --chr {chr} \\\n\
         \x20\x20\x20 --maf {maf} \\\n\
         \x20\x20\x20 --cojo-file \"${{AUTONOMICS_INPUT0}}\" \\\n\
         \x20\x20\x20 --cojo-sblup {lambda:e} \\\n\
         \x20\x20\x20 --cojo-wind {cojo_wind_kb} \\\n\
         \x20\x20\x20 --diff-freq {diff_freq} \\\n\
         \x20\x20\x20 --thread-num {thread_num} \\\n\
         \x20\x20\x20 --out /work/gcta_sblup \\\n\
         \x20\x20\x20> \"${{AUTONOMICS_OUTPUT1}}\" 2>&1\n\
         test -s \"${{AUTONOMICS_OUTPUT0}}\"\n",
        chr = spec.chr,
        maf = spec.maf,
        lambda = spec.lambda,
        cojo_wind_kb = spec.cojo_wind_kb,
        diff_freq = spec.diff_freq,
        thread_num = spec.thread_num,
    );
    command_spec(
        script,
        spec.artifact_prefix.clone(),
        spec.timeout_secs,
        vec![
            ContainerCommandOutputSpec {
                path: "gcta_sblup.sblup.cojo".into(),
                format: Some("gcta_sblup".into()),
            },
            ContainerCommandOutputSpec {
                path: "gcta_sblup.log".into(),
                format: Some("gcta_log".into()),
            },
        ],
        vec![ContainerPanelBundleSpec {
            panel_id: GCTA_REF_BINARY_PANEL.into(),
            mount_path: "/panels/plink_ref".into(),
        }],
    )
}

pub fn fastbat_container_spec(
    spec: &GctaFastbatContainerSpec,
) -> Result<ContainerCommandSpec, String> {
    validate_fastbat(spec)?;
    let script = format!(
        "set -eu\n\
         gcta64 \\\n\
         \x20\x20\x20 --bfile /panels/plink_ref/1000G.EUR.QC.{chr} \\\n\
         \x20\x20\x20 --chr {chr} \\\n\
         \x20\x20\x20 --maf {maf} \\\n\
         \x20\x20\x20 --fastBAT \"${{AUTONOMICS_INPUT0}}\" \\\n\
         \x20\x20\x20 --fastBAT-gene-list /panels/gene_list/glist-hg19.txt \\\n\
         \x20\x20\x20 --fastBAT-wind {gene_flank_kb} \\\n\
         \x20\x20\x20 --fastBAT-ld-cutoff {ld_cutoff} \\\n\
         \x20\x20\x20 --diff-freq {diff_freq} \\\n\
         \x20\x20\x20 --thread-num {thread_num} \\\n\
         \x20\x20\x20 --out /work/gcta_fastbat \\\n\
         \x20\x20\x20> \"${{AUTONOMICS_OUTPUT1}}\" 2>&1\n\
         test -s \"${{AUTONOMICS_OUTPUT0}}\"\n",
        chr = spec.chr,
        maf = spec.maf,
        gene_flank_kb = spec.gene_flank_kb,
        ld_cutoff = spec.ld_cutoff,
        diff_freq = spec.diff_freq,
        thread_num = spec.thread_num,
    );
    command_spec(
        script,
        spec.artifact_prefix.clone(),
        spec.timeout_secs,
        vec![
            ContainerCommandOutputSpec {
                path: "gcta_fastbat.gene.fastbat".into(),
                format: Some("gcta_fastbat".into()),
            },
            ContainerCommandOutputSpec {
                path: "gcta_fastbat.log".into(),
                format: Some("gcta_log".into()),
            },
        ],
        vec![
            ContainerPanelBundleSpec {
                panel_id: GCTA_REF_BINARY_PANEL.into(),
                mount_path: "/panels/plink_ref".into(),
            },
            ContainerPanelBundleSpec {
                panel_id: GCTA_GENE_LIST_PANEL.into(),
                mount_path: "/panels/gene_list".into(),
            },
        ],
    )
}

pub fn acat_container_spec(spec: &GctaAcatContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate_acat(spec)?;
    let script = format!(
        "set -eu\n\
         gcta64 \\\n\
         \x20\x20\x20 --acat \\\n\
         \x20\x20\x20 --snp-list \"${{AUTONOMICS_INPUT0}}\" \\\n\
         \x20\x20\x20 --gene-list /panels/gene_list/glist-hg19.txt \\\n\
         \x20\x20\x20 --max-maf {max_maf} \\\n\
         \x20\x20\x20 --min-mac {min_mac} \\\n\
         \x20\x20\x20 --wind {gene_flank_kb} \\\n\
         \x20\x20\x20 --out /work/gcta_acat_raw \\\n\
         \x20\x20\x20> \"${{AUTONOMICS_OUTPUT1}}\" 2>&1\n\
         test -s /work/gcta_acat_raw\n\
         mv /work/gcta_acat_raw \"${{AUTONOMICS_OUTPUT0}}\"\n",
        max_maf = spec.max_maf,
        min_mac = spec.min_mac,
        gene_flank_kb = spec.gene_flank_kb,
    );
    command_spec(
        script,
        spec.artifact_prefix.clone(),
        spec.timeout_secs,
        vec![
            ContainerCommandOutputSpec {
                path: "gcta_acat.acat".into(),
                format: Some("gcta_acat".into()),
            },
            ContainerCommandOutputSpec {
                path: "gcta_acat.log".into(),
                format: Some("gcta_log".into()),
            },
        ],
        vec![ContainerPanelBundleSpec {
            panel_id: GCTA_GENE_LIST_PANEL.into(),
            mount_path: "/panels/gene_list".into(),
        }],
    )
}

fn plink_binding() -> DataBundleBinding {
    DataBundleBinding::new("plink_ref", GCTA_REF_BINARY_PANEL)
}

fn gene_binding() -> DataBundleBinding {
    DataBundleBinding::new("gene_list", GCTA_GENE_LIST_PANEL)
}

pub struct GctaContainerNodeFactory {
    analysis: GctaAnalysis,
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl GctaContainerNodeFactory {
    pub fn cojo_select(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(GctaAnalysis::CojoSelect, runtime, panel_cache)
    }

    pub fn sblup(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(GctaAnalysis::Sblup, runtime, panel_cache)
    }

    pub fn fastbat(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(GctaAnalysis::Fastbat, runtime, panel_cache)
    }

    pub fn acat(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self::new(GctaAnalysis::Acat, runtime, panel_cache)
    }

    fn new(
        analysis: GctaAnalysis,
        runtime: Arc<dyn PodmanConnection>,
        panel_cache: Arc<PanelCache>,
    ) -> Self {
        Self {
            analysis,
            runtime,
            panel_cache,
        }
    }
}

#[derive(Clone, Copy)]
enum GctaAnalysis {
    CojoSelect,
    Sblup,
    Fastbat,
    Acat,
}

impl GctaAnalysis {
    fn kind(self) -> &'static str {
        match self {
            Self::CojoSelect => GCTA_COJO_SELECT_CONTAINER_KIND,
            Self::Sblup => GCTA_SBLUP_CONTAINER_KIND,
            Self::Fastbat => GCTA_FASTBAT_CONTAINER_KIND,
            Self::Acat => GCTA_ACAT_CONTAINER_KIND,
        }
    }

    fn desc(self) -> &'static str {
        match self {
            Self::CojoSelect => "Runs official GCTA-COJO stepwise selection.",
            Self::Sblup => "Runs official GCTA-COJO SBLUP prediction.",
            Self::Fastbat => "Runs official GCTA-fastBAT gene-based testing.",
            Self::Acat => "Runs official GCTA ACAT-V rare-variant testing.",
        }
    }

    fn doc(self) -> &'static str {
        match self {
            Self::CojoSelect => {
                "Runs official GCTA 1.95.3 `--cojo-slct` against the \
                chromosome-split 1000G EUR PLINK reference. Input is one \
                GCTA-COJO `.ma` File with SNP, A1, A2, freq, b/BETA, se/SE, p \
                and n/N columns. Emits official JMA, LD, conditional-result, \
                and log artifacts. GCTA recommends a GWAS-cohort or large \
                participating-cohort reference where available; this binding is \
                the recorded reproducible 1000G EUR fallback."
            }
            Self::Sblup => {
                "Runs official GCTA 1.95.3 `--cojo-sblup` for one \
                chromosome against the 1000G EUR PLINK reference. Input is one \
                GCTA-COJO `.ma` File; lambda must be supplied by LDSC/GREML or \
                domain knowledge. Emits the official SBLUP SNP-effect table and log."
            }
            Self::Fastbat => {
                "Runs official GCTA-fastBAT 1.95.3 with the official \
                GCTA hg19 gene list and 1000G EUR PLINK LD reference. Input is \
                one GCTA-COJO `.ma` File; output is the official gene-level \
                `.gene.fastbat` result plus the complete log."
            }
            Self::Acat => {
                "Runs official GCTA ACAT-V 1.95.3. Input is one \
                fastGWA File with CHR, SNP, POS, A1, A2, N, AF1, BETA, SE and \
                P columns. The node binds the official GCTA hg19 gene list, \
                requires no LD reference, and emits the official ACAT result and log."
            }
        }
    }

    fn ports(self) -> NodePorts {
        let ports = NodePorts::new().add_input_port_of_type(None, PortType::File);
        match self {
            Self::CojoSelect => ports
                .add_output_port_of_type(None, PortType::File)
                .add_output_port_of_type(None, PortType::File)
                .add_output_port_of_type(None, PortType::File)
                .add_output_port_of_type(None, PortType::File),
            Self::Sblup | Self::Fastbat | Self::Acat => ports
                .add_output_port_of_type(None, PortType::File)
                .add_output_port_of_type(None, PortType::File),
        }
    }

    fn bindings(self) -> Vec<DataBundleBinding> {
        match self {
            Self::CojoSelect | Self::Sblup => vec![plink_binding()],
            Self::Fastbat => vec![plink_binding(), gene_binding()],
            Self::Acat => vec![gene_binding()],
        }
    }

    fn validate_value(self, value: serde_json::Value) -> dag_core::registry::error::Result<()> {
        let result = match self {
            Self::CojoSelect => {
                let spec: GctaCojoSelectContainerSpec = serde_json::from_value(value)?;
                validate_cojo_select(&spec)
            }
            Self::Sblup => {
                let spec: GctaSblupContainerSpec = serde_json::from_value(value)?;
                validate_sblup(&spec)
            }
            Self::Fastbat => {
                let spec: GctaFastbatContainerSpec = serde_json::from_value(value)?;
                validate_fastbat(&spec)
            }
            Self::Acat => {
                let spec: GctaAcatContainerSpec = serde_json::from_value(value)?;
                validate_acat(&spec)
            }
        };
        result.map_err(dag_core::registry::error::Error::Unknown)
    }

    fn container_spec(self, value: serde_json::Value) -> Result<ContainerCommandSpec, String> {
        match self {
            Self::CojoSelect => {
                let spec = serde_json::from_value(value).map_err(|error| error.to_string())?;
                cojo_select_container_spec(&spec)
            }
            Self::Sblup => {
                let spec = serde_json::from_value(value).map_err(|error| error.to_string())?;
                sblup_container_spec(&spec)
            }
            Self::Fastbat => {
                let spec = serde_json::from_value(value).map_err(|error| error.to_string())?;
                fastbat_container_spec(&spec)
            }
            Self::Acat => {
                let spec = serde_json::from_value(value).map_err(|error| error.to_string())?;
                acat_container_spec(&spec)
            }
        }
    }
}

pub struct GctaContainerNode {
    kind: &'static str,
    inner: Box<dyn DagNode>,
}

impl Clone for GctaContainerNode {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for GctaContainerNode {
    fn ports(&self) -> &NodePorts {
        self.inner.ports()
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        self.kind
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        self.inner.execute(ctx, inputs, reporter).await
    }
}

impl NodeFactory for GctaContainerNodeFactory {
    fn kind(&self) -> &'static str {
        self.analysis.kind()
    }

    fn desc(&self) -> &'static str {
        self.analysis.desc()
    }

    fn doc(&self) -> &'static str {
        self.analysis.doc()
    }

    fn spec_schema(&self) -> schemars::Schema {
        match self.analysis {
            GctaAnalysis::CojoSelect => schema_for!(GctaCojoSelectContainerSpec),
            GctaAnalysis::Sblup => schema_for!(GctaSblupContainerSpec),
            GctaAnalysis::Fastbat => schema_for!(GctaFastbatContainerSpec),
            GctaAnalysis::Acat => schema_for!(GctaAcatContainerSpec),
        }
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        self.analysis.bindings()
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        self.analysis.validate_value(spec)?;
        Ok(self.analysis.bindings())
    }

    fn ports(&self) -> NodePorts {
        self.analysis.ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let container_spec = self
            .analysis
            .container_spec(spec)
            .map_err(dag_core::registry::error::Error::Unknown)?;
        let panel_bundles = self
            .analysis
            .bindings()
            .iter()
            .map(|binding| node_ctx.bound_data_bundle(&binding.binding).cloned())
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node = ContainerCommandNode::new_with_catalog_panels(
            self.kind(),
            container_spec,
            runtime,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(GctaContainerNode {
            kind: self.analysis.kind(),
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        self.analysis.validate_value(spec)?;
        Ok(self.analysis.ports())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_official_cojo_contract() {
        let spec = GctaCojoSelectContainerSpec {
            chr: 22,
            maf: default_maf(),
            cojo_p: default_cojo_p(),
            cojo_wind_kb: default_cojo_wind_kb(),
            cojo_collinear: default_cojo_collinear(),
            diff_freq: default_diff_freq(),
            thread_num: default_thread_num(),
            artifact_prefix: default_prefix(),
            timeout_secs: default_timeout(),
        };
        let value = cojo_select_container_spec(&spec).unwrap();
        assert_eq!(
            value.image,
            registry_image(GCTA_ORIGINAL_IMAGE_REPOSITORY, GCTA_ORIGINAL_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(value.panel_bundles.len(), 1);
        assert_eq!(value.outputs.len(), 4);
        let script = value.script.as_deref().unwrap();
        assert!(script.contains("--bfile /panels/plink_ref/1000G.EUR.QC.22"));
        assert!(script.contains("--cojo-slct"));
        assert!(script.contains("--cojo-p 5e-8"));
    }

    #[test]
    fn builds_official_sblup_contract() {
        let spec = GctaSblupContainerSpec {
            chr: 22,
            lambda: 1.33e6,
            maf: default_maf(),
            cojo_wind_kb: default_sblup_wind_kb(),
            diff_freq: default_diff_freq(),
            thread_num: default_thread_num(),
            artifact_prefix: default_prefix(),
            timeout_secs: default_timeout(),
        };
        let value = sblup_container_spec(&spec).unwrap();
        assert_eq!(value.outputs.len(), 2);
        assert!(
            value
                .script
                .as_deref()
                .unwrap()
                .contains("--cojo-sblup 1.33e6")
        );
        assert!(
            value
                .script
                .as_deref()
                .unwrap()
                .contains("--cojo-wind 1000")
        );
    }

    #[test]
    fn builds_official_fastbat_contract() {
        let spec = GctaFastbatContainerSpec {
            chr: 22,
            maf: default_maf(),
            gene_flank_kb: default_fastbat_wind_kb(),
            ld_cutoff: default_fastbat_ld_cutoff(),
            diff_freq: default_diff_freq(),
            thread_num: default_thread_num(),
            artifact_prefix: default_prefix(),
            timeout_secs: default_timeout(),
        };
        let value = fastbat_container_spec(&spec).unwrap();
        assert_eq!(value.panel_bundles.len(), 2);
        assert!(value.script.as_deref().unwrap().contains("--fastBAT"));
        assert!(
            value
                .script
                .as_deref()
                .unwrap()
                .contains("--fastBAT-gene-list /panels/gene_list/glist-hg19.txt")
        );
    }

    #[test]
    fn builds_official_acat_contract_without_plink_panel() {
        let spec = GctaAcatContainerSpec {
            max_maf: default_acat_max_maf(),
            min_mac: default_acat_min_mac(),
            gene_flank_kb: 0,
            artifact_prefix: default_prefix(),
            timeout_secs: default_timeout(),
        };
        let value = acat_container_spec(&spec).unwrap();
        assert_eq!(value.panel_bundles.len(), 1);
        assert_eq!(value.panel_bundles[0].panel_id, GCTA_GENE_LIST_PANEL);
        assert!(value.script.as_deref().unwrap().contains("--acat"));
        assert!(value.script.as_deref().unwrap().contains("--min-mac 20"));
    }

    #[test]
    fn rejects_invalid_parameters() {
        let mut spec = GctaCojoSelectContainerSpec {
            chr: 22,
            maf: default_maf(),
            cojo_p: default_cojo_p(),
            cojo_wind_kb: default_cojo_wind_kb(),
            cojo_collinear: default_cojo_collinear(),
            diff_freq: default_diff_freq(),
            thread_num: default_thread_num(),
            artifact_prefix: default_prefix(),
            timeout_secs: default_timeout(),
        };
        spec.chr = 23;
        assert!(validate_cojo_select(&spec).is_err());
        spec.chr = 22;
        spec.cojo_collinear = 1.0;
        assert!(validate_cojo_select(&spec).is_err());

        let mut sblup = GctaSblupContainerSpec {
            chr: 22,
            lambda: 0.0,
            maf: default_maf(),
            cojo_wind_kb: default_sblup_wind_kb(),
            diff_freq: default_diff_freq(),
            thread_num: default_thread_num(),
            artifact_prefix: default_prefix(),
            timeout_secs: default_timeout(),
        };
        assert!(validate_sblup(&sblup).is_err());
        sblup.lambda = 1.0;
        sblup.artifact_prefix = "relative".into();
        assert!(validate_sblup(&sblup).is_err());

        let mut acat = GctaAcatContainerSpec {
            max_maf: default_acat_max_maf(),
            min_mac: 0,
            gene_flank_kb: 0,
            artifact_prefix: default_prefix(),
            timeout_secs: default_timeout(),
        };
        assert!(validate_acat(&acat).is_err());
        acat.min_mac = 1;
        acat.timeout_secs = 0;
        assert!(validate_acat(&acat).is_err());
    }
}
