//! MAGMA gene-based GWAS analysis nodes (summary-stats pipeline).
//!
//! Four nodes covering the full summary-stats pipeline:
//!
//! | Node | Kind | Input | Output |
//! |------|------|-------|--------|
//! | [`MagmaAnnotateNode`] | `magma_annotate` | gene-loc + snp-loc files | gene annotation DataFrame |
//! | [`MagmaGeneNode`] | `magma_gene` | GWAS pval DataFrame + PLINK ref + annot | gene results DataFrame |
//! | [`MagmaSetNode`] | `magma_set` | gene results DataFrame + set/covar file | set analysis DataFrame |
//! | [`MagmaMetaNode`] | `magma_meta` | ≥2 gene results DataFrames | combined gene results |

use std::sync::Arc;

use arrow_array::{Float64Array, Int32Array, Int64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
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

/// Gene annotation output schema (from annotate node, input to gene node).
fn annot_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("gene_id", DataType::Utf8, false),
        Field::new("chr", DataType::Int32, false),
        Field::new("start", DataType::UInt64, false),
        Field::new("end", DataType::UInt64, false),
        Field::new("snps", DataType::Utf8, false), // semicolon-separated rsids
    ]))
}

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

// =====================================================================
// Node 1: MagmaAnnotateNode
// =====================================================================

/// Config for the annotation node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MagmaAnnotateConfig {
    /// VFS path to gene location file (gene_id chr start end [strand]).
    pub gene_loc: String,
    /// VFS path to SNP location file (.bim or rsid chr pos).
    pub snp_loc: String,
    /// Annotation window in kb (upstream and downstream). Default: 35.
    #[serde(default = "default_window")]
    pub window_kb: f64,
}

fn default_window() -> f64 {
    35.0
}

const ANNOTATE_KIND: &str = "magma_annotate";

fn annotate_ports() -> NodePorts {
    NodePorts::new().add_output_port(Some(annot_schema()))
}

/// Annotation node: maps SNPs to genes based on genomic location.
#[derive(Clone)]
pub struct MagmaAnnotateNode {
    meta: NodePorts,
    config: MagmaAnnotateConfig,
}

pub struct MagmaAnnotateNodeFactory;

impl NodeFactory for MagmaAnnotateNodeFactory {
    fn kind(&self) -> &'static str {
        ANNOTATE_KIND
    }
    fn desc(&self) -> &'static str {
        "MAGMA gene-SNP annotation: maps SNPs to genes by genomic location."
    }
    fn doc(&self) -> &'static str {
        "Maps SNPs to genes based on genomic location ± window. Reads gene-loc \
        and snp-loc files, outputs a DataFrame with gene_id, chr, start, end, \
        and semicolon-separated SNP IDs per gene."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MagmaAnnotateConfig)
    }
    fn ports(&self) -> NodePorts {
        annotate_ports()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config = serde_json::from_value(spec)?;
        Ok(Box::new(MagmaAnnotateNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        _ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<MagmaAnnotateConfig>(spec, "magma_annotate")?;
        let out = "magma_annotate_result";
        let code = vec![
            format!("# MAGMA gene annotation"),
            format!("system2(\"magma\", c("),
            format!(
                "  \"--annotate\", \"--window\", \"--snp-loc\", \"{}\",",
                s.snp_loc
            ),
            format!("  \"--gene-loc\", \"{}\",", s.gene_loc),
            format!("  \"--out\", \"magma_annotation\""),
            format!("))"),
            format!("# NOTE: Output written to magma_annotation.genes.annot"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
}

impl MagmaAnnotateNode {
    pub fn new(config: MagmaAnnotateConfig) -> Self {
        Self {
            meta: annotate_ports(),
            config,
        }
    }
}

#[async_trait]
impl DagNode for MagmaAnnotateNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        ANNOTATE_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let gene_path = stage_vfs_file(node_ctx, &self.config.gene_loc).await?;
        let snp_path = stage_vfs_file(node_ctx, &self.config.snp_loc).await?;
        let genes =
            magma::annotation::read_gene_loc(gene_path.as_ref()).map_err(MagmaNodeError::from)?;
        let snps =
            magma::annotation::read_snp_loc(snp_path.as_ref()).map_err(MagmaNodeError::from)?;
        let window_bp = (self.config.window_kb * 1000.0) as i64;
        let annot = magma::annotation::annotate(&genes, &snps, window_bp, window_bp)
            .map_err(MagmaNodeError::from)?;

        let batch = build_annot_batch(&annot)?;
        let df = node_ctx
            .session()
            .read_batch(batch)
            .map_err(MagmaNodeError::from)?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

fn build_annot_batch(
    annot: &magma::annotation::GeneAnnotation,
) -> Result<RecordBatch, MagmaNodeError> {
    let n = annot.genes.len();
    let mut gene_ids = Vec::with_capacity(n);
    let mut chrs = Vec::with_capacity(n);
    let mut starts = Vec::with_capacity(n);
    let mut ends = Vec::with_capacity(n);
    let mut snps_str = Vec::with_capacity(n);

    for g in &annot.genes {
        if g.snps.is_empty() {
            continue;
        }
        gene_ids.push(g.id.clone());
        chrs.push(g.chr);
        starts.push(g.start);
        ends.push(g.end);
        snps_str.push(g.snps.join(";"));
    }

    let batch = RecordBatch::try_new(
        annot_schema(),
        vec![
            Arc::new(StringArray::from(gene_ids)),
            Arc::new(Int32Array::from(chrs)),
            Arc::new(arrow_array::UInt64Array::from(starts)), // u64 → Arrow uses u32 for UInt64? No.
            Arc::new(arrow_array::UInt64Array::from(ends)),
            Arc::new(StringArray::from(snps_str)),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Node 2: MagmaGeneNode
// =====================================================================

/// Config for the gene analysis node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MagmaGeneConfig {
    /// VFS path to .genes.annot file.
    pub gene_annot: String,
    /// VFS prefix for a PLINK .bed/.bim/.fam reference panel.
    #[serde(default = "default_reference_prefix")]
    pub reference_prefix: String,
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

fn default_reference_prefix() -> String {
    DEFAULT_REFERENCE_PREFIX.to_string()
}

const VFS_PREFIX: &str = "vfs://";

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

/// Default 1000 Genomes East Asian panel deployed under `/mnt/data/magma`.
const DEFAULT_REFERENCE_PREFIX: &str = "vfs:///data/magma/references/g1000_eas/g1000_eas";

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
        loads the PLINK reference panel for LD estimation, and computes \
        gene-level test statistics using the SNP-wise mean model (Imhof). \
        Outputs gene_id, chr, start, end, n_snps, n_param, n, zstat, pval."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MagmaGeneConfig)
    }
    fn ports(&self) -> NodePorts {
        gene_ports()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config = serde_json::from_value(spec)?;
        Ok(Box::new(MagmaGeneNode::new(config)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<MagmaGeneConfig>(spec, "magma_gene")?;
        let input = input_0(ctx).to_string();
        let out = ctx.output_var.to_string();
        let n_args = match &s.fixed_n {
            Some(n) => format!(" --sample-n {}", n),
            None => format!(" --n-col {}", s.n_col),
        };
        let code = vec![
            format!("# MAGMA gene-level analysis"),
            format!("# Write sumstats to temp file first"),
            format!("tmp_sumstats <- tempfile(fileext = \".sumstats\")"),
            format!("data.table::fwrite({input}, tmp_sumstats, sep = \"\\t\")"),
            format!("system2(\"magma\", c("),
            format!("  \"--gene-results\", \"{}\",", s.gene_annot),
            format!("  \"--bfile\", \"<plink_bed_prefix>\","),
            format!(
                "  \"--pval\", tmp_sumstats, usecols=\"{} {}\",",
                s.snp_col, s.pval_col
            ),
            format!("  \"{n_args}\","),
            format!("  \"--out\", \"{out}\""),
            format!("))"),
            format!("# NOTE: Output in {out}.genes.raw and {out}.genes.out"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
}

impl MagmaGeneNode {
    pub fn new(config: MagmaGeneConfig) -> Self {
        Self {
            meta: gene_ports(),
            config,
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
        let df = &inputs[0].data;
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

        // Load PLINK + annotation
        let mut bed = open_plink_vfs(node_ctx, &self.config.reference_prefix).await?;
        let annot_path = stage_vfs_file(node_ctx, &self.config.gene_annot).await?;
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
        covariate file. Outputs variable, type, n_genes, beta, beta_std, se, pval."
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

    fn codegen_r(
        &self,
        _spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        let input = ctx
            .input_vars
            .first()
            .cloned()
            .unwrap_or_else(|| "__missing_input".into());
        let out = ctx.output_var.to_string();
        let code = vec![
            format!("# MAGMA gene-set analysis"),
            format!("# Input: {input} (gene-level results)"),
            format!("system2(\"magma\", c("),
            format!("  \"--gene-results\", \"<gene_raw_file>\","),
            format!("  \"--set-annot\", \"<set_annotation_file>\","),
            format!("  \"--out\", \"{out}\""),
            format!("))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
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
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // Load gene data from .genes.raw file or construct from DataFrame
        let gene_data = if let Some(ref raw_path) = self.config.gene_raw {
            let staged = stage_vfs_file(node_ctx, raw_path).await?;
            magma::setanalysis::GeneRawData::read(staged.as_ref()).map_err(MagmaNodeError::from)?
        } else if !inputs.is_empty() {
            gene_results_to_raw(&inputs[0].data).await?
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
/// This is used when the set node receives gene results from the gene node
/// instead of a .genes.raw file. Correlations default to 0 (identity matrix).
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
    /// VFS paths to .genes.raw files for each cohort.
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
        Outputs combined gene results."
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

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<MagmaMetaConfig>(spec, "magma_meta")?;
        let out = ctx.output_var.to_string();
        let cohort_files = s
            .cohort_files
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(" ");
        let code = vec![
            format!("# MAGMA meta-analysis"),
            format!("system2(\"magma\", c("),
            format!("  \"--meta\", {cohort_files},"),
            format!("  \"--out\", \"{out}\""),
            format!("))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
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
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.config.cohort_files.len() < 2 {
            return Err(MagmaNodeError::Magma(magma::MagmaError::Input(
                "magma_meta requires at least 2 cohort files".into(),
            ))
            .into());
        }

        let staged_cohorts = futures::future::try_join_all(
            self.config
                .cohort_files
                .iter()
                .map(|path| stage_vfs_file(node_ctx, path)),
        )
        .await?;
        let cohorts: Vec<magma::setanalysis::GeneRawData> = staged_cohorts
            .iter()
            .map(|path| magma::setanalysis::GeneRawData::read(path.as_ref()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(MagmaNodeError::from)?;

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
        let source_root = magma_data_dir();
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
        NodeCtx {
            runtime_env: SessionContext::new().runtime_env(),
            opendal: Some(opendal),
            global_sem: None,
        }
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
    async fn e2e_annotate_node() {
        let mut node = MagmaAnnotateNode::new(MagmaAnnotateConfig {
            gene_loc: vpath("gene_loc.txt"),
            snp_loc: vpath("sim_geno.bim"),
            window_kb: 35.0,
        });
        let res = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("annotate should succeed");

        let df = &res[&0];
        let count = df.clone().count().await.unwrap();
        assert_eq!(count, 20, "should have 20 genes");
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
        let input = vec![NodeInput { port: 0, data: df }];

        let mut node = MagmaGeneNode::new(MagmaGeneConfig {
            gene_annot: vpath("annot.genes.annot"),
            reference_prefix: vpath("sim_geno"),
            snp_col: "rsid".into(),
            pval_col: "pval".into(),
            n_col: "n".into(),
            fixed_n: Some(50000),
        });

        let res = node
            .execute(
                &node_ctx(),
                &input,
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("gene analysis should succeed");

        let df = &res[&0];
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

        let df = &res[&0];
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

        let df = &res[&0];
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

        let df = &res[&0];
        let count = df.clone().count().await.unwrap();
        assert_eq!(count, 20, "should have 20 genes");
    }

    #[tokio::test]
    async fn e2e_full_pipeline() {
        // Full pipeline: annotate → gene → set
        let ctx = node_ctx();

        // Step 1: Annotation
        let mut annotate = MagmaAnnotateNode::new(MagmaAnnotateConfig {
            gene_loc: vpath("gene_loc.txt"),
            snp_loc: vpath("sim_geno.bim"),
            window_kb: 35.0,
        });
        let _annot_res = annotate
            .execute(&ctx, &[], &dag_core::dag::node_event::NodeReporter::noop())
            .await
            .expect("annotate should succeed");

        // Step 2: Gene analysis (using pre-computed annot.genes.annot)
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

        let mut gene_node = MagmaGeneNode::new(MagmaGeneConfig {
            gene_annot: vpath("annot.genes.annot"),
            reference_prefix: vpath("sim_geno"),
            snp_col: "rsid".into(),
            pval_col: "pval".into(),
            n_col: "n".into(),
            fixed_n: Some(50000),
        });
        let gene_res = gene_node
            .execute(
                &ctx,
                &[NodeInput { port: 0, data: df }],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("gene analysis should succeed");

        // Verify gene output
        let gene_df = &gene_res[&0];
        let gene_count = gene_df.clone().count().await.unwrap();
        assert_eq!(gene_count, 20);

        // Step 3: Set analysis (using .genes.raw from golden for correlation matrix)
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

        let set_df = &set_res[&0];
        let set_count = set_df.clone().count().await.unwrap();
        assert_eq!(set_count, 6);

        eprintln!("Full pipeline: annotate -> gene -> set completed successfully");
    }
}
