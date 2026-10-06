//! MAGMA gene-based GWAS analysis nodes (summary-stats pipeline).
//!
//! Three nodes covering the retained summary-stats pipeline. Official MAGMA
//! annotation runs through `the `magma_annotate` manifest plugin`.
//!
//! | Node | Kind | Input | Output |
//! |------|------|-------|--------|
//! | [`MagmaGeneNode`] | `magma_gene` | GWAS pval DataFrame + panel bundle | gene results DataFrame |
//! | [`MagmaSetNode`] | `magma_set` | gene results DataFrame + set/covar file | set analysis DataFrame |
//! | [`MagmaMetaNode`] | `magma_meta` | ≥2 gene results DataFrames | combined gene results |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, Int64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use dag_core::node::{DagNode, DataBundle, DataBundleBinding, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

// =====================================================================
// Error
// =====================================================================

#[derive(Debug, Error)]
pub enum MagmaNodeError {
    #[error("MAGMA computation failed: {0}")]
    Magma(#[from] magma::MagmaError),
    #[error("MAGMA VFS read failed for {path}: {message}")]
    Vfs { path: String, message: String },
    #[error("MAGMA reference bundle error for {reference}: {message}")]
    ReferenceBundle { reference: String, message: String },
    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("DataFusion error: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
}

impl ::dag_core::dag::NodeError for MagmaNodeError {
    fn node_type(&self) -> &str {
        "magma"
    }
}

// =====================================================================
// Shared schemas
// =====================================================================

/// Gene analysis output schema (from gene node, input to set/meta node).
fn gene_results_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("gene_id", DataType::Utf8, false),
        Field::new("chr", DataType::Int32, false),
        Field::new("start", DataType::UInt32, false),
        Field::new("end", DataType::UInt32, false),
        Field::new("n_snps", DataType::Int32, false),
        Field::new("n_param", DataType::Int32, false),
        Field::new("n", DataType::Int64, false),
        Field::new("zstat", DataType::Float64, false),
        Field::new("pval", DataType::Float64, false),
    ]))
}

/// Gene-set analysis output schema.
fn set_results_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("variable", DataType::Utf8, false),
        Field::new("type", DataType::Utf8, false),
        Field::new("n_genes", DataType::Int32, false),
        Field::new("beta", DataType::Float64, false),
        Field::new("beta_std", DataType::Float64, false),
        Field::new("se", DataType::Float64, false),
        Field::new("pval", DataType::Float64, false),
    ]))
}

/// GWAS sumstats input schema (port 0 of gene node).
fn gwas_input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("rsid", DataType::Utf8, false),
        Field::new("pval", DataType::Float64, false),
        Field::new("n", DataType::Int64, true),
    ]))
}

fn default_window() -> f64 {
    35.0
}

// =====================================================================
// MagmaGeneNode
// =====================================================================

/// Config for the gene analysis node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MagmaGeneConfig {
    /// Semantic ID of a deployed panel bundle, such as `g1000_eas`.
    ///
    /// If omitted, the node derives `g1000_<population>` from the population.
    #[serde(default)]
    pub reference: Option<String>,
    /// Genome build required from the reference bundle.
    #[serde(default = "default_genome_build")]
    pub genome_build: String,
    /// Population ancestry required from the reference bundle. Implicit
    /// references support `AFR`, `AMR`, `EAS`, `EUR`, and `SAS`.
    #[serde(default = "default_population")]
    pub population: String,
    /// NCBI gene-location release bundled with the reference panel.
    #[serde(default = "default_gene_release")]
    pub gene_release: String,
    /// Gene annotation window in kb. It must match the precomputed annotation.
    #[serde(default = "default_window")]
    pub window_kb: f64,
    /// SNP ID column name in the input GWAS DataFrame. Default: "rsid".
    #[serde(default = "default_rsid_col")]
    pub snp_col: String,
    /// P-value column name. Default: "pval".
    #[serde(default = "default_pval_col")]
    pub pval_col: String,
    /// Sample size column name (optional). If absent, uses `fixed_n`.
    #[serde(default = "default_n_col")]
    pub n_col: String,
    /// Fixed sample size (used if n_col is absent or null).
    #[serde(default)]
    pub fixed_n: Option<i64>,
}

fn default_rsid_col() -> String {
    "rsid".to_string()
}
fn default_pval_col() -> String {
    "pval".to_string()
}
fn default_n_col() -> String {
    "n".to_string()
}

fn default_gene_release() -> String {
    "NCBI37.3".to_string()
}

fn default_genome_build() -> String {
    "GRCh37".to_string()
}

fn default_population() -> String {
    "EAS".to_string()
}

const VFS_PREFIX: &str = "vfs://";
const BUNDLE_MANIFEST: &str = "bundle.json";
const SUPPORTED_IMPLICIT_POPULATIONS: [&str; 5] = ["AFR", "AMR", "EAS", "EUR", "SAS"];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MagmaPanelBundle {
    schema_version: u8,
    id: String,
    genome_build: String,
    population: String,
    plink_prefix: String,
    gene_annotation: MagmaPanelGeneAnnotation,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MagmaPanelGeneAnnotation {
    release: String,
    window_kb: f64,
    path: String,
    sha256: String,
}

#[derive(Clone, Debug)]
struct ResolvedMagmaReference {
    prefix_vpath: String,
    annotation_vpath: String,
    expected_sha256: String,
}

/// Read a file through the runtime's configured VFS mount catalog.
async fn read_vfs_bytes(node_ctx: &NodeCtx, raw_path: &str) -> Result<Vec<u8>, MagmaNodeError> {
    let Some(rest) = raw_path.strip_prefix(VFS_PREFIX) else {
        return Err(MagmaNodeError::Vfs {
            path: raw_path.to_string(),
            message: "expected a vfs:// path".into(),
        });
    };
    let path = vfs::OpendalFileStorage::normalize_path(rest);
    let storage = node_ctx
        .opendal
        .as_ref()
        .ok_or_else(|| MagmaNodeError::Vfs {
            path: raw_path.to_string(),
            message: "no VFS storage is configured".into(),
        })?;
    if !storage.is_mounted(&path) {
        return Err(MagmaNodeError::Vfs {
            path: raw_path.to_string(),
            message: "path is not covered by a VFS mount".into(),
        });
    }

    let op = storage.resolve(&path);
    let key = storage.resolve_path(&path);
    let size = op
        .stat(&key)
        .await
        .map_err(|e| MagmaNodeError::Vfs {
            path: raw_path.to_string(),
            message: e.to_string(),
        })?
        .content_length();
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    let mut reader = op
        .reader_with(&key)
        .concurrent(8)
        .chunk(8 * 1024 * 1024)
        .await
        .map_err(|e| MagmaNodeError::Vfs {
            path: raw_path.to_string(),
            message: e.to_string(),
        })?
        .into_futures_async_read(..)
        .await
        .map_err(|e| MagmaNodeError::Vfs {
            path: raw_path.to_string(),
            message: e.to_string(),
        })?;
    futures::io::AsyncReadExt::read_to_end(&mut reader, &mut bytes)
        .await
        .map_err(|e| MagmaNodeError::Vfs {
            path: raw_path.to_string(),
            message: e.to_string(),
        })?;
    Ok(bytes)
}

/// Stage a text input for the existing synchronous MAGMA parsers.
///
/// The bytes still originate from VFS; this only isolates parser I/O from the
/// backend and removes the staging directory when execution finishes.
struct VfsStagedFile {
    _guard: tempfile::TempDir,
    path: std::path::PathBuf,
}

fn bundle_error(reference: &str, message: impl Into<String>) -> MagmaNodeError {
    MagmaNodeError::ReferenceBundle {
        reference: reference.to_string(),
        message: message.into(),
    }
}

fn valid_reference_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn resolve_requested_reference(
    reference: Option<&str>,
    population: &str,
) -> Result<(String, String), MagmaNodeError> {
    let population = population.trim().to_ascii_uppercase();
    let Some(reference) = reference.map(str::trim).filter(|value| !value.is_empty()) else {
        if !SUPPORTED_IMPLICIT_POPULATIONS.contains(&population.as_str()) {
            return Err(MagmaNodeError::ReferenceBundle {
                reference: population.clone(),
                message: format!(
                    "implicit references support only {}",
                    SUPPORTED_IMPLICIT_POPULATIONS.join(", ")
                ),
            });
        }
        return Ok((
            format!("g1000_{}", population.to_ascii_lowercase()),
            population,
        ));
    };

    if !valid_reference_id(reference) {
        return Err(MagmaNodeError::ReferenceBundle {
            reference: reference.to_string(),
            message: "reference IDs may contain only ASCII letters, digits, '_', '-', and '.', and must start with a letter or digit".into(),
        });
    }
    Ok((reference.to_string(), population))
}

fn join_bundle_path(reference: &str, root: &str, relative: &str) -> Result<String, MagmaNodeError> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.starts_with("vfs://")
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(bundle_error(
            reference,
            format!("bundle path '{relative}' must be relative to the bundle and cannot escape it"),
        ));
    }
    Ok(format!("{root}/{relative}"))
}

async fn resolve_reference_bundle(
    node_ctx: &NodeCtx,
    runtime_bundle: &DataBundle,
    reference: &str,
    genome_build: &str,
    population: &str,
    gene_release: &str,
    window_kb: f64,
) -> Result<(MagmaPanelBundle, ResolvedMagmaReference), MagmaNodeError> {
    if !valid_reference_id(reference) {
        return Err(bundle_error(
            reference,
            "reference IDs may contain only ASCII letters, digits, '_', '-', and '.', and must start with a letter or digit",
        ));
    }

    let root = runtime_bundle.vpath.trim_end_matches('/').to_string();
    let manifest_path = format!("vfs://{root}/{BUNDLE_MANIFEST}");
    let manifest_bytes = read_vfs_bytes(node_ctx, &manifest_path).await?;
    let bundle: MagmaPanelBundle = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| bundle_error(reference, format!("invalid {BUNDLE_MANIFEST}: {e}")))?;

    if bundle.schema_version != 1 {
        return Err(bundle_error(
            reference,
            format!("unsupported schema_version {}", bundle.schema_version),
        ));
    }
    if bundle.id != reference {
        return Err(bundle_error(
            reference,
            format!(
                "manifest ID '{}' does not match requested reference",
                bundle.id
            ),
        ));
    }
    if bundle.plink_prefix.is_empty() {
        return Err(bundle_error(reference, "plink_prefix cannot be empty"));
    }
    if bundle.genome_build.trim().is_empty() {
        return Err(bundle_error(reference, "genome_build cannot be empty"));
    }
    if bundle.population.trim().is_empty() {
        return Err(bundle_error(reference, "population cannot be empty"));
    }
    if bundle.genome_build != genome_build {
        return Err(bundle_error(
            reference,
            format!(
                "bundle uses genome build '{}', but '{}' was requested",
                bundle.genome_build, genome_build
            ),
        ));
    }
    if bundle.population != population {
        return Err(bundle_error(
            reference,
            format!(
                "bundle is for population '{}', but '{}' was requested",
                bundle.population, population
            ),
        ));
    }

    let annotation = &bundle.gene_annotation;
    if annotation.release.trim().is_empty() {
        return Err(bundle_error(
            reference,
            "gene annotation release cannot be empty",
        ));
    }
    if !(annotation.window_kb.is_finite() && annotation.window_kb > 0.0) {
        return Err(bundle_error(
            reference,
            "gene annotation window_kb must be finite and positive",
        ));
    }
    if annotation.release != gene_release {
        return Err(bundle_error(
            reference,
            format!(
                "bundle provides gene release '{}', but '{}' was requested",
                annotation.release, gene_release
            ),
        ));
    }
    if (annotation.window_kb - window_kb).abs() > f64::EPSILON {
        return Err(bundle_error(
            reference,
            format!(
                "bundle provides a {:.1} kb annotation, but {:.1} kb was requested",
                annotation.window_kb, window_kb
            ),
        ));
    }

    let prefix_vpath = format!(
        "vfs://{}",
        join_bundle_path(reference, &root, &bundle.plink_prefix)?
    );
    let annotation_vpath = format!(
        "vfs://{}",
        join_bundle_path(reference, &root, &annotation.path)?
    );
    let expected_sha256 = annotation.sha256.clone();
    if expected_sha256.len() != 64
        || expected_sha256
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit())
    {
        return Err(bundle_error(
            reference,
            "gene annotation sha256 must be 64 hex characters",
        ));
    }

    Ok((
        bundle,
        ResolvedMagmaReference {
            prefix_vpath,
            annotation_vpath,
            expected_sha256,
        },
    ))
}

fn validate_annotation_checksum(
    reference: &str,
    expected: &str,
    bytes: &[u8],
) -> Result<(), MagmaNodeError> {
    let actual = Sha256::digest(bytes);
    let actual = actual
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(bundle_error(
            reference,
            format!("gene annotation checksum mismatch: expected {expected}, got {actual}"),
        ));
    }
    Ok(())
}

impl AsRef<std::path::Path> for VfsStagedFile {
    fn as_ref(&self) -> &std::path::Path {
        &self.path
    }
}

async fn stage_vfs_file(
    node_ctx: &NodeCtx,
    raw_path: &str,
) -> Result<VfsStagedFile, MagmaNodeError> {
    let bytes = read_vfs_bytes(node_ctx, raw_path).await?;
    stage_vfs_bytes(raw_path, bytes).await
}

async fn stage_vfs_bytes(raw_path: &str, bytes: Vec<u8>) -> Result<VfsStagedFile, MagmaNodeError> {
    let name = raw_path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| MagmaNodeError::Vfs {
            path: raw_path.to_string(),
            message: "path has no file name".into(),
        })?;
    let dir = tempfile::tempdir().map_err(|e| MagmaNodeError::Vfs {
        path: raw_path.to_string(),
        message: format!("create staging directory: {e}"),
    })?;
    let staged = dir.path().join(name);
    std::fs::write(&staged, bytes).map_err(|e| MagmaNodeError::Vfs {
        path: raw_path.to_string(),
        message: format!("stage file: {e}"),
    })?;
    Ok(VfsStagedFile {
        path: staged,
        _guard: dir,
    })
}

/// Read all three PLINK files through VFS and build an in-memory BED reader.
async fn open_plink_vfs(
    node_ctx: &NodeCtx,
    prefix: &str,
) -> Result<magma::plink::BedFile, MagmaNodeError> {
    let (bed, bim, fam) = futures::future::try_join3(
        read_vfs_bytes(node_ctx, &format!("{prefix}.bed")),
        read_vfs_bytes(node_ctx, &format!("{prefix}.bim")),
        read_vfs_bytes(node_ctx, &format!("{prefix}.fam")),
    )
    .await?;
    let bim = String::from_utf8_lossy(&bim).into_owned();
    let fam = String::from_utf8_lossy(&fam).into_owned();
    magma::plink::BedFile::from_bytes(bed, &bim, &fam).map_err(MagmaNodeError::from)
}

const GENE_KIND: &str = "magma_gene";

fn gene_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(gwas_input_schema()))
        .add_output_port(Some(gene_results_schema()))
}

/// Gene analysis node: computes gene-level Z-statistics from SNP p-values.
#[derive(Clone)]
pub struct MagmaGeneNode {
    meta: NodePorts,
    config: MagmaGeneConfig,
    reference_bundle: DataBundle,
}

pub struct MagmaGeneNodeFactory;

impl NodeFactory for MagmaGeneNodeFactory {
    fn kind(&self) -> &'static str {
        GENE_KIND
    }
    fn desc(&self) -> &'static str {
        "MAGMA gene analysis: SNP p-values + reference LD → gene Z-statistics."
    }
    fn doc(&self) -> &'static str {
        "Takes a GWAS summary statistics DataFrame (rsid, pval, optional n), \
        resolves a versioned panel bundle (PLINK LD reference plus matching \
        gene annotation), and computes gene-level test statistics using the \
        SNP-wise mean model (Imhof). \
        Outputs gene_id, chr, start, end, n_snps, n_param, n, zstat, pval."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MagmaGeneConfig)
    }
    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let config: MagmaGeneConfig = serde_json::from_value(spec)?;
        let (reference, _) =
            resolve_requested_reference(config.reference.as_deref(), &config.population)
                .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(vec![DataBundleBinding::new("reference", reference)])
    }
    fn ports(&self) -> NodePorts {
        gene_ports()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config = serde_json::from_value(spec)?;
        Ok(Box::new(MagmaGeneNode::new(
            config,
            node_ctx.bound_data_bundle("reference")?.clone(),
        )))
    }
}

impl MagmaGeneNode {
    pub fn new(config: MagmaGeneConfig, reference_bundle: DataBundle) -> Self {
        Self {
            meta: gene_ports(),
            config,
            reference_bundle,
        }
    }
}

#[async_trait]
impl DagNode for MagmaGeneNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        GENE_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if inputs.is_empty() {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(
                "magma_gene requires a GWAS DataFrame input".into(),
            ))
            .into());
        }

        // Collect the GWAS DataFrame into batches
        let df = inputs[0].dataframe()?;
        let batches = df.clone().collect().await.map_err(MagmaNodeError::from)?;

        // Extract rsid, pval, n from the batches
        let rsids = extract_string_col(&batches, &self.config.snp_col)?;
        let pvals = extract_f64_col(&batches, &self.config.pval_col)?;
        let ns: Vec<Option<i64>> = if batches
            .iter()
            .any(|b| b.schema().field_with_name(&self.config.n_col).is_ok())
        {
            extract_i64_col(&batches, &self.config.n_col)?
        } else {
            vec![self.config.fixed_n; rsids.len()]
        };

        // Build SnpPvalData
        let mut snp_pvals = std::collections::HashMap::new();
        for (i, rsid) in rsids.iter().enumerate() {
            snp_pvals.insert(rsid.clone(), (pvals[i], ns[i]));
        }
        let pval_data = magma::geneinput::SnpPvalData { snps: snp_pvals };

        // The panel and its gene annotation are resolved as one versioned bundle.
        let (reference, population) =
            resolve_requested_reference(self.config.reference.as_deref(), &self.config.population)?;
        let (_bundle, resolved) = resolve_reference_bundle(
            node_ctx,
            &self.reference_bundle,
            &reference,
            &self.config.genome_build,
            &population,
            &self.config.gene_release,
            self.config.window_kb,
        )
        .await?;
        let mut bed = open_plink_vfs(node_ctx, &resolved.prefix_vpath).await?;
        let annot_bytes = read_vfs_bytes(node_ctx, &resolved.annotation_vpath).await?;
        validate_annotation_checksum(&reference, &resolved.expected_sha256, &annot_bytes)?;
        let annot_path = stage_vfs_bytes(&resolved.annotation_vpath, annot_bytes).await?;
        let annot =
            magma::geneinput::GeneAnnot::read(annot_path.as_ref()).map_err(MagmaNodeError::from)?;

        // Run gene analysis
        let config = magma::geneanalysis::PvalAnalysisConfig {
            fixed_n: self.config.fixed_n,
            ..Default::default()
        };
        let results = magma::geneanalysis::analyze_pval(&mut bed, &annot, &pval_data, &config)
            .map_err(MagmaNodeError::from)?;

        // Build output batch
        let batch = build_gene_results_batch(&results)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(MagmaNodeError::from)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

fn build_gene_results_batch(
    results: &[magma::geneanalysis::GeneResult],
) -> Result<RecordBatch, MagmaNodeError> {
    let gene_ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    let chrs: Vec<i32> = results.iter().map(|r| r.chr).collect();
    let starts: Vec<u32> = results.iter().map(|r| r.start as u32).collect();
    let ends: Vec<u32> = results.iter().map(|r| r.end as u32).collect();
    let n_snps: Vec<i32> = results.iter().map(|r| r.n_snps as i32).collect();
    let n_param: Vec<i32> = results.iter().map(|r| r.n_param as i32).collect();
    let ns: Vec<i64> = results.iter().map(|r| r.n).collect();
    let zstats: Vec<f64> = results.iter().map(|r| r.zstat).collect();
    let pvals: Vec<f64> = results.iter().map(|r| r.pval).collect();

    let batch = RecordBatch::try_new(
        gene_results_schema(),
        vec![
            Arc::new(StringArray::from(gene_ids)),
            Arc::new(Int32Array::from(chrs)),
            Arc::new(UInt32Array::from(starts)),
            Arc::new(UInt32Array::from(ends)),
            Arc::new(Int32Array::from(n_snps)),
            Arc::new(Int32Array::from(n_param)),
            Arc::new(Int64Array::from(ns)),
            Arc::new(Float64Array::from(zstats)),
            Arc::new(Float64Array::from(pvals)),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Node 3: MagmaSetNode
// =====================================================================

/// Config for the gene-set / gene-property analysis node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MagmaSetConfig {
    /// Analysis type: "set" for gene-set, "covar" for gene-property.
    #[serde(default = "default_analysis_type")]
    pub analysis_type: String,
    /// VFS path to gene-set annotation file (for analysis_type="set").
    pub set_annot: Option<String>,
    /// VFS path to gene covariate file (for analysis_type="covar").
    pub gene_covar: Option<String>,
    /// VFS path to .genes.raw file (alternative to DataFrame input).
    pub gene_raw: Option<String>,
    /// Column index for gene ID in set file (0-based). Default: 1.
    #[serde(default = "default_col_gene")]
    pub col_gene: usize,
    /// Column index for set name in set file (0-based). Default: 0.
    #[serde(default = "default_col_set")]
    pub col_set: usize,
}

fn default_analysis_type() -> String {
    "set".to_string()
}
fn default_col_gene() -> usize {
    1
}
fn default_col_set() -> usize {
    0
}

const SET_KIND: &str = "magma_set";

fn set_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(gene_results_schema()))
        .add_output_port(Some(set_results_schema()))
}

/// Gene-set / gene-property analysis node.
#[derive(Clone)]
pub struct MagmaSetNode {
    meta: NodePorts,
    config: MagmaSetConfig,
}

pub struct MagmaSetNodeFactory;

impl NodeFactory for MagmaSetNodeFactory {
    fn kind(&self) -> &'static str {
        SET_KIND
    }
    fn desc(&self) -> &'static str {
        "MAGMA gene-set/property analysis: competitive regression on gene sets."
    }
    fn doc(&self) -> &'static str {
        "Takes gene results (from magma_gene or .genes.raw file) and runs \
        competitive regression analysis. For analysis_type='set', reads a \
        gene-set annotation file. For analysis_type='covar', reads a gene \
        covariate file. NOTE: with a DataFrame input (no gene_raw path) the \
        gene-gene correlation matrix is unavailable — the identity matrix, a \
        constant placeholder MAC, and zero coordinates are substituted, so \
        those results are exploratory only; prefer a .genes.raw file built \
        from a real LD reference. Outputs variable, type, n_genes, beta, \
        beta_std, se, pval."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MagmaSetConfig)
    }
    fn ports(&self) -> NodePorts {
        set_ports()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config = serde_json::from_value(spec)?;
        Ok(Box::new(MagmaSetNode::new(config)))
    }
}

impl MagmaSetNode {
    pub fn new(config: MagmaSetConfig) -> Self {
        Self {
            meta: set_ports(),
            config,
        }
    }
}

#[async_trait]
impl DagNode for MagmaSetNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        SET_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // Load gene data from .genes.raw file or construct from DataFrame
        let gene_data = if let Some(ref raw_path) = self.config.gene_raw {
            let staged = stage_vfs_file(node_ctx, raw_path).await?;
            magma::setanalysis::GeneRawData::read(staged.as_ref()).map_err(MagmaNodeError::from)?
        } else if !inputs.is_empty() {
            reporter.warn(
                "magma_set: the DataFrame input carries no gene-gene correlation matrix, \
                 no real MAC, and no gene coordinates — start/end default to 0, mac to 100, \
                 and R to the identity. Without a .genes.raw file (gene_raw) produced from a \
                 real LD reference the competitive-regression results are exploratory only.",
            );
            gene_results_to_raw(inputs[0].dataframe()?).await?
        } else {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(
                "magma_set requires either gene_raw path or DataFrame input".into(),
            ))
            .into());
        };

        // Run analysis
        let results = match self.config.analysis_type.as_str() {
            "set" => {
                let set_path = self.config.set_annot.as_ref().ok_or_else(|| {
                    MagmaNodeError::Magma(magma::MagmaError::Input(
                        "set_annot path required for analysis_type='set'".into(),
                    ))
                })?;
                let staged = stage_vfs_file(node_ctx, set_path).await?;
                let set_data = magma::setanalysis::GeneSetData::read(
                    staged.as_ref(),
                    &gene_data,
                    self.config.col_gene,
                    self.config.col_set,
                )
                .map_err(MagmaNodeError::from)?;
                magma::setanalysis::analyze_gene_sets(&gene_data, &set_data)
                    .map_err(MagmaNodeError::from)?
            }
            "covar" => {
                let covar_path = self.config.gene_covar.as_ref().ok_or_else(|| {
                    MagmaNodeError::Magma(magma::MagmaError::Input(
                        "gene_covar path required for analysis_type='covar'".into(),
                    ))
                })?;
                let staged = stage_vfs_file(node_ctx, covar_path).await?;
                let covar_data =
                    magma::setanalysis::GeneCovarData::read(staged.as_ref(), &gene_data)
                        .map_err(MagmaNodeError::from)?;
                magma::setanalysis::analyze_gene_covar(&gene_data, &covar_data)
                    .map_err(MagmaNodeError::from)?
            }
            other => {
                return Err(MagmaNodeError::Magma(magma::MagmaError::Input(format!(
                    "unknown analysis_type: {other}"
                )))
                .into());
            }
        };

        let batch = build_set_results_batch(&results)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(MagmaNodeError::from)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// Convert a gene results DataFrame to GeneRawData (without correlations).
///
/// This is used when the set/meta node receives gene results from the gene
/// node instead of a `.genes.raw` file. The gene-results schema carries no
/// gene-gene correlations, no MAC, and no gene coordinates, so they are
/// FABRICATED here: `corrs` default to 0 (identity matrix), `mac` to a
/// constant 100, and `start`/`end` to 0. Importing real LD-derived
/// correlations/MAC for a DataFrame input is a scheme-level adaptation that
/// is intentionally NOT implemented; until then, competitive-regression
/// results built from this conversion are exploratory only (the caller
/// emits a runtime WARN). See `MagmaSetNodeFactory::doc`.
async fn gene_results_to_raw(
    df: &datafusion::dataframe::DataFrame,
) -> Result<magma::setanalysis::GeneRawData, MagmaNodeError> {
    let batches = df.clone().collect().await.map_err(MagmaNodeError::from)?;
    let gene_ids = extract_string_col(&batches, "gene_id")?;
    let chrs = extract_i32_col(&batches, "chr")?;
    let n_snps = extract_i32_col(&batches, "n_snps")?;
    let n_param = extract_i32_col(&batches, "n_param")?;
    let ns = extract_i64_col(&batches, "n")?
        .into_iter()
        .map(|v| v.unwrap_or(0))
        .collect::<Vec<_>>();
    let zstats = extract_f64_col(&batches, "zstat")?;

    let genes: Vec<magma::setanalysis::GeneRawEntry> = gene_ids
        .iter()
        .enumerate()
        .map(|(i, id)| magma::setanalysis::GeneRawEntry {
            id: id.clone(),
            chr: chrs[i],
            start: 0,
            end: 0,
            n_snps: n_snps[i] as usize,
            n_param: n_param[i] as usize,
            n: ns[i],
            mac: 100.0, // default placeholder
            zstat: zstats[i],
        })
        .collect();

    // No correlations available from DataFrame — use empty (identity matrix)
    let corrs: Vec<Vec<f64>> = (0..genes.len()).map(|i| vec![0.0; i]).collect();

    Ok(magma::setanalysis::GeneRawData { genes, corrs })
}

fn build_set_results_batch(
    results: &[magma::setanalysis::SetResult],
) -> Result<RecordBatch, MagmaNodeError> {
    let vars: Vec<&str> = results.iter().map(|r| r.variable.as_str()).collect();
    let types_owned: Vec<String> = results.iter().map(|r| r.var_type.to_string()).collect();
    let types_refs: Vec<&str> = types_owned.iter().map(|s| s.as_str()).collect();
    let n_genes: Vec<i32> = results.iter().map(|r| r.n_genes as i32).collect();
    let betas: Vec<f64> = results.iter().map(|r| r.beta).collect();
    let beta_stds: Vec<f64> = results.iter().map(|r| r.beta_std).collect();
    let ses: Vec<f64> = results.iter().map(|r| r.se).collect();
    let pvals: Vec<f64> = results.iter().map(|r| r.pval).collect();

    let batch = RecordBatch::try_new(
        set_results_schema(),
        vec![
            Arc::new(StringArray::from(vars)),
            Arc::new(StringArray::from(types_refs)),
            Arc::new(Int32Array::from(n_genes)),
            Arc::new(Float64Array::from(betas)),
            Arc::new(Float64Array::from(beta_stds)),
            Arc::new(Float64Array::from(ses)),
            Arc::new(Float64Array::from(pvals)),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Node 4: MagmaMetaNode
// =====================================================================

/// Config for the meta-analysis node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MagmaMetaConfig {
    /// VFS paths to .genes.raw files for each cohort. Used only when no
    /// cohort DataFrames are wired to the variadic input port; ignored
    /// otherwise.
    #[serde(default)]
    pub cohort_files: Vec<String>,
    /// Optional weights (one per cohort). Default: √N.
    #[serde(default)]
    pub weights: Option<Vec<f64>>,
}

const META_KIND: &str = "magma_meta";

fn meta_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(gene_results_schema()))
        .set_fixed_input(false)
        .add_output_port(Some(gene_results_schema()))
}

/// Meta-analysis node: combines gene Z-statistics from multiple cohorts.
#[derive(Clone)]
pub struct MagmaMetaNode {
    meta: NodePorts,
    config: MagmaMetaConfig,
}

pub struct MagmaMetaNodeFactory;

impl NodeFactory for MagmaMetaNodeFactory {
    fn kind(&self) -> &'static str {
        META_KIND
    }
    fn desc(&self) -> &'static str {
        "MAGMA meta-analysis: combine gene results from multiple cohorts."
    }
    fn doc(&self) -> &'static str {
        "Combines gene-level Z-statistics from multiple cohorts using \
        inverse-variance weighted combination (√N weighting by default). \
        Cohorts are the wired input DataFrames (gene-results schema, ≥2 \
        required); when no inputs are wired, .genes.raw paths from the \
        cohort_files config are used instead. Outputs combined gene results."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MagmaMetaConfig)
    }
    fn ports(&self) -> NodePorts {
        meta_ports()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config = serde_json::from_value(spec)?;
        Ok(Box::new(MagmaMetaNode::new(config)))
    }
}

impl MagmaMetaNode {
    pub fn new(config: MagmaMetaConfig) -> Self {
        Self {
            meta: meta_ports(),
            config,
        }
    }
}

#[async_trait]
impl DagNode for MagmaMetaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        META_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // Cohort inputs: wired DataFrames take precedence over the config
        // paths, closing the port↔execution contract (the node advertises a
        // variadic gene-results input but previously ignored it).
        let cohorts: Vec<magma::setanalysis::GeneRawData> = if inputs.len() >= 2 {
            reporter.info(format!(
                "magma_meta: combining {} connected cohort tables",
                inputs.len()
            ));
            let cohort_futures = inputs
                .iter()
                .map(|input| input.dataframe().map(gene_results_to_raw))
                .collect::<Result<Vec<_>, _>>()?;
            futures::future::try_join_all(cohort_futures).await?
        } else if inputs.len() == 1 {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(
                "magma_meta has 1 wired cohort input; wire at least 2, or none \
                 to fall back to the cohort_files config paths"
                    .into(),
            ))
            .into());
        } else if self.config.cohort_files.len() >= 2 {
            let staged_cohorts = futures::future::try_join_all(
                self.config
                    .cohort_files
                    .iter()
                    .map(|path| stage_vfs_file(node_ctx, path)),
            )
            .await?;
            staged_cohorts
                .iter()
                .map(|path| magma::setanalysis::GeneRawData::read(path.as_ref()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(MagmaNodeError::from)?
        } else {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(
                "magma_meta requires at least 2 cohorts: wire ≥2 gene-results \
                 DataFrames to the input port or list ≥2 cohort_files paths"
                    .into(),
            ))
            .into());
        };

        let meta = magma::meta::meta_analyze(&cohorts, self.config.weights.as_deref(), None)
            .map_err(MagmaNodeError::from)?;

        // Build output: convert GeneRawData to GeneResult format
        let results: Vec<magma::geneanalysis::GeneResult> = meta
            .genes
            .iter()
            .map(|g| magma::geneanalysis::GeneResult {
                id: g.id.clone(),
                chr: g.chr,
                start: g.start,
                end: g.end,
                n_snps: g.n_snps,
                n_param: g.n_param,
                n: g.n,
                zstat: g.zstat,
                pval: magma::stats::zstat_to_pval(g.zstat),
            })
            .collect();

        let batch = build_gene_results_batch(&results)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(MagmaNodeError::from)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// Column extraction helpers
// =====================================================================

fn extract_f64_col(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, MagmaNodeError> {
    let schema = batches
        .first()
        .ok_or_else(|| MagmaNodeError::Magma(magma::MagmaError::Input("no batches".into())))?
        .schema();
    let idx = schema.index_of(name).map_err(|_| {
        MagmaNodeError::Magma(magma::MagmaError::Input(format!("missing column '{name}'")))
    })?;

    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        if let Some(arr) = col.as_any().downcast_ref::<Float64Array>() {
            for v in arr.iter() {
                out.push(v.unwrap_or(f64::NAN));
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
            for v in arr.iter() {
                out.push(v.map(|v| v as f64).unwrap_or(f64::NAN));
            }
        } else {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(format!(
                "column '{name}' is not numeric"
            ))));
        }
    }
    Ok(out)
}

fn extract_string_col(batches: &[RecordBatch], name: &str) -> Result<Vec<String>, MagmaNodeError> {
    let schema = batches
        .first()
        .ok_or_else(|| MagmaNodeError::Magma(magma::MagmaError::Input("no batches".into())))?
        .schema();
    let idx = schema.index_of(name).map_err(|_| {
        MagmaNodeError::Magma(magma::MagmaError::Input(format!("missing column '{name}'")))
    })?;

    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<arrow_array::LargeStringArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else if let Some(arr) = col.as_any().downcast_ref::<arrow_array::StringViewArray>() {
            for v in arr.iter() {
                out.push(v.unwrap_or("").to_string());
            }
        } else {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(format!(
                "column '{name}' is not a string"
            ))));
        }
    }
    Ok(out)
}

fn extract_i64_col(
    batches: &[RecordBatch],
    name: &str,
) -> Result<Vec<Option<i64>>, MagmaNodeError> {
    let schema = batches
        .first()
        .ok_or_else(|| MagmaNodeError::Magma(magma::MagmaError::Input("no batches".into())))?
        .schema();
    let idx = schema.index_of(name).map_err(|_| {
        MagmaNodeError::Magma(magma::MagmaError::Input(format!("missing column '{name}'")))
    })?;

    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
            for v in arr.iter() {
                out.push(v);
            }
        } else {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(format!(
                "column '{name}' is not Int64"
            ))));
        }
    }
    Ok(out)
}

fn extract_i32_col(batches: &[RecordBatch], name: &str) -> Result<Vec<i32>, MagmaNodeError> {
    let schema = batches
        .first()
        .ok_or_else(|| MagmaNodeError::Magma(magma::MagmaError::Input("no batches".into())))?
        .schema();
    let idx = schema.index_of(name).map_err(|_| {
        MagmaNodeError::Magma(magma::MagmaError::Input(format!("missing column '{name}'")))
    })?;

    let mut out = Vec::new();
    for batch in batches {
        let col = batch.column(idx);
        if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
            for v in arr.iter() {
                out.push(v.unwrap_or(0));
            }
        } else {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(format!(
                "column '{name}' is not Int32"
            ))));
        }
    }
    Ok(out)
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;
    use vfs::{BackendDefinition, MountDefinition, VfsManifest};

    fn node_ctx() -> NodeCtx {
        node_ctx_with_root(magma_data_dir()).0
    }

    fn node_ctx_with_root(source_root: std::path::PathBuf) -> (NodeCtx, tempfile::TempDir) {
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "magma-test".into(),
                config: vfs::BackendConfig::local("/"),
            }],
            mount: vec![MountDefinition {
                path: "/data/magma".into(),
                backend: "magma-test".into(),
                source: source_root.to_string_lossy().to_string(),
                read_only: true,
            }],
        };
        let mounts =
            std::sync::Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
        let scratch = tempfile::tempdir().unwrap();
        let opendal =
            std::sync::Arc::new(vfs::OpendalFileStorage::with_mounts(scratch.path(), mounts));
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), Some(opendal));
        (ctx, scratch)
    }

    fn magma_data_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../bio_crates/magma/tests/data")
            .canonicalize()
            .expect("MAGMA fixture directory should exist")
    }

    fn vpath(name: &str) -> String {
        format!("vfs:///data/magma/{name}")
    }

    fn panel_bundle_ctx() -> (NodeCtx, tempfile::TempDir, String) {
        let bundle_mount = tempfile::tempdir().unwrap();
        let (ctx, _scratch) = node_ctx_with_root(bundle_mount.path().to_path_buf());
        let source = magma_data_dir();
        let reference = "sim_panel".to_string();
        let bundle_root = bundle_mount.path().join("references").join(&reference);
        let annotation_dir = bundle_root.join("annotations");
        std::fs::create_dir_all(&annotation_dir).unwrap();
        for extension in ["bed", "bim", "fam"] {
            std::fs::copy(
                source.join(format!("sim_geno.{extension}")),
                bundle_root.join(format!("sim_panel.{extension}")),
            )
            .unwrap();
        }
        let annotation_path = annotation_dir.join("sim_panel.NCBI37.3.window35.genes.annot");
        std::fs::copy(source.join("annot.genes.annot"), &annotation_path).unwrap();
        let annotation_bytes = std::fs::read(&annotation_path).unwrap();
        let checksum = Sha256::digest(&annotation_bytes);
        let checksum = checksum
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        std::fs::write(
            bundle_root.join(BUNDLE_MANIFEST),
            format!(
                r#"{{
                  "schema_version": 1,
                  "id": "{reference}",
                  "genome_build": "GRCh37",
                  "population": "SIM",
                  "plink_prefix": "sim_panel",
                  "gene_annotation": {{
                    "release": "NCBI37.3",
                    "window_kb": 35,
                    "path": "annotations/sim_panel.NCBI37.3.window35.genes.annot",
                    "sha256": "{checksum}"
                  }}
                }}"#
            ),
        )
        .unwrap();

        (ctx, bundle_mount, reference)
    }

    #[test]
    fn magma_gene_config_uses_reference_defaults() {
        let config: MagmaGeneConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.reference, None);
        assert_eq!(config.genome_build, "GRCh37");
        assert_eq!(config.population, "EAS");
        assert_eq!(config.gene_release, "NCBI37.3");
        assert_eq!(config.window_kb, 35.0);

        let error =
            serde_json::from_str::<MagmaGeneConfig>(r#"{ "gene_annot": "vfs:///external.annot" }"#)
                .unwrap_err();
        assert!(error.to_string().contains("unknown field `gene_annot`"));
    }

    #[test]
    fn magma_gene_switches_implicit_reference_by_population() {
        let (reference, population) =
            resolve_requested_reference(None, "eur").expect("EUR should resolve");
        assert_eq!(reference, "g1000_eur");
        assert_eq!(population, "EUR");

        let (reference, population) =
            resolve_requested_reference(None, "AFR").expect("AFR should resolve");
        assert_eq!(reference, "g1000_afr");
        assert_eq!(population, "AFR");

        let (reference, population) = resolve_requested_reference(Some("ukb_eur "), "EUR")
            .expect("explicit references should remain supported");
        assert_eq!(reference, "ukb_eur");
        assert_eq!(population, "EUR");

        let error = resolve_requested_reference(None, "FIN").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("implicit references support only")
        );
    }

    #[test]
    fn magma_gene_rejects_annotation_checksum_mismatch() {
        let error = validate_annotation_checksum(
            "test_panel",
            "0000000000000000000000000000000000000000000000000000000000000000",
            b"annotation",
        )
        .unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"));
    }

    #[tokio::test]
    async fn resolves_deployed_1000g_population_bundles() {
        let source_root = std::path::Path::new("/mnt/data/magma/resources");
        if !source_root
            .join("references/g1000_sas/bundle.json")
            .exists()
        {
            return;
        }

        let (ctx, _scratch) = node_ctx_with_root(source_root.to_path_buf());
        for population in SUPPORTED_IMPLICIT_POPULATIONS {
            let (reference, normalized_population) =
                resolve_requested_reference(None, population).unwrap();
            let (_bundle, resolved) = resolve_reference_bundle(
                &ctx,
                &DataBundle::new(
                    reference.clone(),
                    reference.clone(),
                    format!("/data/magma/references/{reference}"),
                ),
                &reference,
                "GRCh37",
                &normalized_population,
                "NCBI37.3",
                35.0,
            )
            .await
            .unwrap();
            let bytes = read_vfs_bytes(&ctx, &resolved.annotation_vpath)
                .await
                .unwrap();
            validate_annotation_checksum(&reference, &resolved.expected_sha256, &bytes).unwrap();
            let staged = stage_vfs_bytes(&resolved.annotation_vpath, bytes)
                .await
                .unwrap();
            magma::geneinput::GeneAnnot::read(staged.as_ref()).unwrap();
        }
    }

    fn gwas_batch(rsids: Vec<String>, pvals: Vec<f64>, n: i64) -> RecordBatch {
        let ns: Vec<i64> = vec![n; rsids.len()];
        let schema = Arc::new(Schema::new(vec![
            Field::new("rsid", DataType::Utf8, false),
            Field::new("pval", DataType::Float64, false),
            Field::new("n", DataType::Int64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(rsids)),
                Arc::new(Float64Array::from(pvals)),
                Arc::new(Int64Array::from(ns)),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn e2e_gene_node() {
        let dir = magma_data_dir();

        // Read GWAS p-values
        let pval_data = magma::geneinput::SnpPvalData::read(
            &dir.join("gwas_pval.txt"),
            "SNP",
            "P",
            Some("N"),
            None,
        )
        .unwrap();
        let rsids: Vec<String> = pval_data.snps.keys().cloned().collect();
        let pvals: Vec<f64> = rsids
            .iter()
            .map(|r| pval_data.snps.get(r).unwrap().0)
            .collect();

        let batch = gwas_batch(rsids, pvals, 50000);
        let df = node_ctx().session().read_batch(batch).unwrap();
        let input = vec![NodeInput::new_dataframe(0, df)];

        let (ctx, _bundle, reference) = panel_bundle_ctx();
        let runtime_bundle = DataBundle::new(
            reference.clone(),
            reference.clone(),
            format!("/data/magma/references/{reference}"),
        );
        let mut node = MagmaGeneNode::new(
            MagmaGeneConfig {
                reference: Some(reference.clone()),
                genome_build: "GRCh37".into(),
                population: "SIM".into(),
                gene_release: "NCBI37.3".into(),
                window_kb: 35.0,
                snp_col: "rsid".into(),
                pval_col: "pval".into(),
                n_col: "n".into(),
                fixed_n: Some(50000),
            },
            runtime_bundle,
        );

        let res = node
            .execute(
                &ctx,
                &input,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("gene analysis should succeed");

        let df = res.dataframe(0).unwrap();
        let count = df.clone().count().await.unwrap();
        assert_eq!(count, 20, "should have 20 genes");

        // Verify Z-statistic matches golden output
        let batches = df.clone().collect().await.unwrap();
        let zstats = extract_f64_col(&batches, "zstat").unwrap();
        let gene_ids = extract_string_col(&batches, "gene_id").unwrap();

        // Golden: gene 10000 ZSTAT=1.6612
        let idx = gene_ids.iter().position(|g| g == "10000").unwrap();
        assert!(
            (zstats[idx] - 1.6612).abs() < 0.15,
            "gene 10000 ZSTAT mismatch: {} vs 1.6612",
            zstats[idx]
        );
    }

    #[tokio::test]
    async fn e2e_set_node() {
        let mut node = MagmaSetNode::new(MagmaSetConfig {
            analysis_type: "set".into(),
            set_annot: Some(vpath("gene_sets.txt")),
            gene_covar: None,
            gene_raw: Some(vpath("gene_pval.genes.raw")),
            col_gene: 1,
            col_set: 0,
        });

        let res = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("set analysis should succeed");

        let df = res.dataframe(0).unwrap();
        let count = df.clone().count().await.unwrap();
        // Should have 6 sets (SetAllGenes discarded as it contains all genes)
        assert_eq!(count, 6, "should have 6 gene sets");

        let batches = df.clone().collect().await.unwrap();
        let pvals = extract_f64_col(&batches, "pval").unwrap();
        for &p in &pvals {
            assert!((0.0..=1.0).contains(&p), "p-value out of range: {p}");
        }
    }

    #[tokio::test]
    async fn e2e_covar_node() {
        let mut node = MagmaSetNode::new(MagmaSetConfig {
            analysis_type: "covar".into(),
            set_annot: None,
            gene_covar: Some(vpath("gene_covar.txt")),
            gene_raw: Some(vpath("gene_pval.genes.raw")),
            col_gene: 1,
            col_set: 0,
        });

        let res = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("covar analysis should succeed");

        let df = res.dataframe(0).unwrap();
        let count = df.clone().count().await.unwrap();
        assert_eq!(count, 3, "should have 3 covariates");
    }

    #[tokio::test]
    async fn e2e_meta_node() {
        let raw_path = vpath("gene_pval.genes.raw");
        let mut node = MagmaMetaNode::new(MagmaMetaConfig {
            cohort_files: vec![raw_path.clone(), raw_path],
            weights: None,
        });

        let res = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("meta analysis should succeed");

        let df = res.dataframe(0).unwrap();
        let count = df.clone().count().await.unwrap();
        assert_eq!(count, 20, "should have 20 genes");
    }

    #[tokio::test]
    async fn meta_node_combines_wired_cohort_tables() {
        let ctx = node_ctx();
        let make_cohort = |gene_id: &str, n: i64, zstat: f64| {
            let batch = RecordBatch::try_new(
                gene_results_schema(),
                vec![
                    Arc::new(StringArray::from(vec![gene_id.to_string()])),
                    Arc::new(Int32Array::from(vec![1])),
                    Arc::new(UInt32Array::from(vec![1000u32])),
                    Arc::new(UInt32Array::from(vec![1500u32])),
                    Arc::new(Int32Array::from(vec![10])),
                    Arc::new(Int32Array::from(vec![6])),
                    Arc::new(Int64Array::from(vec![n])),
                    Arc::new(Float64Array::from(vec![zstat])),
                    Arc::new(Float64Array::from(vec![0.5])),
                ],
            )
            .unwrap();
            ctx.session().read_batch(batch).unwrap()
        };

        // Port↔execution contract: wired cohort tables are the cohort input
        // even with an empty cohort_files config.
        let mut node = MagmaMetaNode::new(MagmaMetaConfig {
            cohort_files: vec![],
            weights: None,
        });
        let res = node
            .execute(
                &ctx,
                &[
                    NodeInput::new_dataframe(0, make_cohort("geneA", 100, 1.0)),
                    NodeInput::new_dataframe(1, make_cohort("geneA", 400, 2.0)),
                ],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("wired cohort tables should be meta-analyzed");

        let df = res.dataframe(0).unwrap();
        let batches = df.clone().collect().await.unwrap();
        let ids = extract_string_col(&batches, "gene_id").unwrap();
        let zstats = extract_f64_col(&batches, "zstat").unwrap();
        let ns = extract_i64_col(&batches, "n").unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "geneA");
        // Hand-computed √N weights: (10·1 + 20·2) / √(10² + 20²) = 50/√500.
        assert!(
            (zstats[0] - 2.236_067_977_499_79).abs() < 1e-12,
            "combined zstat: got {}",
            zstats[0]
        );
        assert_eq!(ns[0], Some(500));

        // A single wired cohort cannot be meta-analyzed and is rejected.
        let error = node
            .execute(
                &ctx,
                &[NodeInput::new_dataframe(0, make_cohort("geneA", 100, 1.0))],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("1 wired cohort input"),
            "{error}"
        );

        // No inputs and no cohort_files is rejected.
        let error = node
            .execute(&ctx, &[], &dag_core::dag::node_event::NodeReporter::noop())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("at least 2 cohorts"), "{error}");
    }

    #[tokio::test]
    async fn e2e_gene_set_pipeline() {
        // Retained native pipeline: official annotation output → gene → set.
        let (bundle_ctx, _bundle, reference) = panel_bundle_ctx();
        let ctx = node_ctx();

        // Gene analysis consumes a precomputed official-layout annotation.
        let pval_data = magma::geneinput::SnpPvalData::read(
            &magma_data_dir().join("gwas_pval.txt"),
            "SNP",
            "P",
            Some("N"),
            None,
        )
        .unwrap();
        let rsids: Vec<String> = pval_data.snps.keys().cloned().collect();
        let pvals: Vec<f64> = rsids
            .iter()
            .map(|r| pval_data.snps.get(r).unwrap().0)
            .collect();
        let batch = gwas_batch(rsids, pvals, 50000);
        let df = ctx.session().read_batch(batch).unwrap();

        let mut gene_node = MagmaGeneNode::new(
            MagmaGeneConfig {
                reference: Some(reference.clone()),
                genome_build: "GRCh37".into(),
                population: "SIM".into(),
                gene_release: "NCBI37.3".into(),
                window_kb: 35.0,
                snp_col: "rsid".into(),
                pval_col: "pval".into(),
                n_col: "n".into(),
                fixed_n: Some(50000),
            },
            DataBundle::new(
                reference.clone(),
                reference.clone(),
                format!("/data/magma/references/{reference}"),
            ),
        );
        let gene_res = gene_node
            .execute(
                &bundle_ctx,
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("gene analysis should succeed");

        // Verify gene output
        let gene_df = gene_res.dataframe(0).unwrap();
        let gene_count = gene_df.clone().count().await.unwrap();
        assert_eq!(gene_count, 20);

        // Set analysis uses the golden `.genes.raw` correlation matrix.
        let mut set_node = MagmaSetNode::new(MagmaSetConfig {
            analysis_type: "set".into(),
            set_annot: Some(vpath("gene_sets.txt")),
            gene_covar: None,
            gene_raw: Some(vpath("gene_pval.genes.raw")),
            col_gene: 1,
            col_set: 0,
        });
        let set_res = set_node
            .execute(&ctx, &[], &dag_core::dag::node_event::NodeReporter::noop())
            .await
            .expect("set analysis should succeed");

        let set_df = set_res.dataframe(0).unwrap();
        let set_count = set_df.clone().count().await.unwrap();
        assert_eq!(set_count, 6);

        eprintln!("Retained pipeline: gene -> set completed successfully");
    }
}
