//! Multi-omic concordance node: joins RNA DE, tissue proteomics and CSF
//! proteomics tables on gene symbol, filters to direction-consistent genes,
//! and ranks the survivors by the absolute effect-size product. The node is a
//! pure DataFusion computation with no container runtime overhead, because
//! the inputs are tabular, well-defined, and do not require any external tool.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use futures::StreamExt;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileRef, PortType};
use dag_core::{
    dag::node_event::NodeReporter,
    dag::{DagError, graph::PortOutputs},
};

const KIND: &str = "multiomic_concordance";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PValueTransform {
    #[default]
    None,
    NegativeLog10,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ModalityColumns {
    pub gene_col: String,
    pub effect_col: String,
    pub pvalue_col: String,
    #[serde(default)]
    pub pvalue_transform: PValueTransform,
}

fn default_columns() -> ModalityColumns {
    ModalityColumns {
        gene_col: "gene_symbol".into(),
        effect_col: "log_fc".into(),
        pvalue_col: "pvalue".into(),
        pvalue_transform: PValueTransform::None,
    }
}

fn default_gene_delimiter() -> String {
    ";".into()
}

fn default_artifact_prefix() -> String {
    "/artifacts/multiomic_concordance".into()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MultiomicConcordanceSpec {
    #[serde(default = "default_columns")]
    pub rna: ModalityColumns,
    #[serde(default = "default_columns")]
    pub tissue_protein: ModalityColumns,
    #[serde(default = "default_columns")]
    pub csf_protein: ModalityColumns,
    /// Separator used when one protein row carries several symbols, for
    /// example `A;B`. Each symbol inherits that row's effect and p-value.
    #[serde(default = "default_gene_delimiter")]
    pub gene_delimiter: String,
    /// Optional p-value cutoff (raw) applied to each input independently
    /// before the join. Genes with a non-finite effect or p-value above the
    /// cutoff are dropped per modality. Use `null` to keep all rows.
    #[serde(default)]
    pub per_input_p_cutoff: Option<f64>,
    /// When true, require direction agreement across all three modalities.
    /// When false, only magnitude agreement is required.
    #[serde(default = "default_true")]
    pub require_sign_consistency: bool,
    /// VFS prefix used to publish ranked candidates, filter counts and the
    /// JSON run report.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum ConcordanceError {
    #[error("multiomic_concordance spec invalid: {0}")]
    Invalid(String),
    #[error("multiomic_concordance failed: {0}")]
    Runtime(String),
}

impl From<ConcordanceError> for DagError {
    fn from(error: ConcordanceError) -> Self {
        DagError::Schedule(error.to_string())
    }
}

pub struct MultiomicConcordanceNodeFactory;

pub struct MultiomicConcordanceNode {
    ports: NodePorts,
    spec: MultiomicConcordanceSpec,
}

impl Clone for MultiomicConcordanceNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            spec: self.spec.clone(),
        }
    }
}

pub fn validate(spec: &MultiomicConcordanceSpec) -> Result<(), String> {
    for (label, columns) in [
        ("rna", &spec.rna),
        ("tissue_protein", &spec.tissue_protein),
        ("csf_protein", &spec.csf_protein),
    ] {
        for (kind, value) in [
            ("gene_col", &columns.gene_col),
            ("effect_col", &columns.effect_col),
            ("pvalue_col", &columns.pvalue_col),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{label}.{kind} cannot be empty"));
            }
        }
    }
    if spec.gene_delimiter.trim().is_empty() {
        return Err("gene_delimiter cannot be empty".into());
    }
    if let Some(cutoff) = spec.per_input_p_cutoff {
        if !cutoff.is_finite() || cutoff <= 0.0 || cutoff >= 1.0 {
            return Err("per_input_p_cutoff must lie strictly between 0 and 1".into());
        }
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    Ok(())
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::DataFrame, "rna_effects")
        .add_input_port_of_type_with_label(None, PortType::DataFrame, "tissue_protein_effects")
        .add_input_port_of_type_with_label(None, PortType::DataFrame, "csf_protein_effects")
        .add_optional_input_port_of_type(PortType::DataFrame)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for MultiomicConcordanceNodeFactory {
    fn kind(&self) -> &'static str {
        KIND
    }
    fn desc(&self) -> &'static str {
        "Joins RNA, tissue proteomics and CSF proteomics tables, filters to direction-consistent genes, and ranks survivors by the absolute effect-size product."
    }
    fn doc(&self) -> &'static str {
        "Three DataFrame inputs carry RNA DE, tissue proteomics and CSF proteomics effects. Column names and p-value encoding are configured per modality; an optional fourth DataFrame maps input IDs to gene symbols. Multi-symbol cells are exploded with a configurable delimiter, raw p-values are cutoff-filtered, direction agreement is optional, and ranking uses the absolute effect-size product. The node publishes ranked candidates, detailed filter counts, matched symbols and a fingerprinted JSON run report."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MultiomicConcordanceSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: MultiomicConcordanceSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(Box::new(MultiomicConcordanceNode {
            ports: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for MultiomicConcordanceNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if !matches!(inputs.len(), 3 | 4) {
            return Err(DagError::Schedule(format!(
                "multiomic_concordance requires 3 analysis inputs plus an optional ID mapping (got {})",
                inputs.len()
            )));
        }
        let mut ports: Vec<Option<Vec<RecordBatch>>> = vec![None; 4];
        for input in inputs {
            let batches = input.dataframe()?.clone().collect().await.map_err(|e| {
                ConcordanceError::Runtime(format!("port {} collect failed: {}", input.port, e))
            })?;
            let port = usize::from(input.port);
            if port >= ports.len() || ports[port].is_some() {
                return Err(ConcordanceError::Invalid(format!(
                    "duplicate or invalid input port {}",
                    input.port
                ))
                .into());
            }
            ports[port] = Some(batches);
        }
        let mapping = match ports[3].take() {
            Some(batches) => Some(parse_id_mapping(&batches)?),
            None => None,
        };
        let column_sets = [
            &self.spec.rna,
            &self.spec.tissue_protein,
            &self.spec.csf_protein,
        ];
        let mut tables = Vec::with_capacity(3);
        for index in 0..3 {
            let batches = ports[index].take().ok_or_else(|| {
                ConcordanceError::Invalid(format!("input port {index} is required"))
            })?;
            tables.push(build_table(
                index,
                &batches,
                column_sets[index],
                mapping.as_ref(),
                &self.spec,
            )?);
        }
        let joined = join_and_score(&tables, &self.spec, reporter)?;
        let mapping_entries = mapping.as_ref().map_or(0, BTreeMap::len);
        let prefix = self.spec.artifact_prefix.trim_end_matches('/').to_string();
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            ConcordanceError::Runtime(
                "multiomic_concordance requires a registered runtime VFS".into(),
            )
        })?;
        let fingerprints = write_outputs(
            ctx,
            &prefix,
            storage.as_ref(),
            &self.spec,
            mapping_entries,
            &joined,
        )
        .await?;
        Ok(joined_outputs(&prefix, fingerprints))
    }
}

#[derive(Debug, Clone)]
struct ModalityRow {
    symbol: String,
    effect: f64,
    pvalue: f64,
}

#[derive(Debug)]
struct ModalityTable {
    label: String,
    rows: Vec<ModalityRow>,
    source_rows: u64,
}

fn parse_id_mapping(batches: &[RecordBatch]) -> Result<BTreeMap<String, String>, ConcordanceError> {
    let mut mapping = BTreeMap::new();
    for batch in batches {
        let schema = batch.schema();
        let input_idx = schema.index_of("input_id").map_err(|_| {
            ConcordanceError::Runtime("ID mapping is missing column `input_id`".into())
        })?;
        let gene_idx = schema.index_of("gene_symbol").map_err(|_| {
            ConcordanceError::Runtime("ID mapping is missing column `gene_symbol`".into())
        })?;
        let inputs = batch
            .column(input_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| ConcordanceError::Runtime("mapping `input_id` is not Utf8".into()))?;
        let genes = batch
            .column(gene_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| ConcordanceError::Runtime("mapping `gene_symbol` is not Utf8".into()))?;
        for row in 0..batch.num_rows() {
            if inputs.is_null(row) || genes.is_null(row) {
                continue;
            }
            let input_id = inputs.value(row).trim();
            let gene_symbol = genes.value(row).trim();
            if !input_id.is_empty() && !gene_symbol.is_empty() {
                mapping
                    .entry(input_id.to_string())
                    .or_insert(gene_symbol.to_string());
            }
        }
    }
    Ok(mapping)
}

fn build_table(
    index: usize,
    batches: &[RecordBatch],
    columns: &ModalityColumns,
    mapping: Option<&BTreeMap<String, String>>,
    spec: &MultiomicConcordanceSpec,
) -> Result<ModalityTable, ConcordanceError> {
    let label = match index {
        0 => "rna",
        1 => "tissue_protein",
        2 => "csf_protein",
        _ => unreachable!(),
    };
    let mut rows = Vec::new();
    let mut source_rows = 0u64;
    for batch in batches {
        let schema = batch.schema();
        let gene_idx = schema.index_of(&columns.gene_col).map_err(|_| {
            ConcordanceError::Runtime(format!(
                "{label} table missing column `{}`",
                columns.gene_col
            ))
        })?;
        let effect_idx = schema.index_of(&columns.effect_col).map_err(|_| {
            ConcordanceError::Runtime(format!(
                "{label} table missing column `{}`",
                columns.effect_col
            ))
        })?;
        let pvalue_idx = schema.index_of(&columns.pvalue_col).map_err(|_| {
            ConcordanceError::Runtime(format!(
                "{label} table missing column `{}`",
                columns.pvalue_col
            ))
        })?;
        let genes = batch
            .column(gene_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                ConcordanceError::Runtime(format!(
                    "{label} column `{}` is not Utf8",
                    columns.gene_col
                ))
            })?;
        let effects = batch
            .column(effect_idx)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| {
                ConcordanceError::Runtime(format!(
                    "{label} column `{}` is not Float64",
                    columns.effect_col
                ))
            })?;
        let pvalues = batch
            .column(pvalue_idx)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| {
                ConcordanceError::Runtime(format!(
                    "{label} column `{}` is not Float64",
                    columns.pvalue_col
                ))
            })?;
        for row in 0..batch.num_rows() {
            if genes.is_null(row) {
                continue;
            }
            let raw_symbol = genes.value(row).trim();
            if raw_symbol.is_empty() {
                continue;
            }
            source_rows += 1;
            let effect = if effects.is_null(row) {
                f64::NAN
            } else {
                effects.value(row)
            };
            let pvalue = if pvalues.is_null(row) {
                f64::NAN
            } else {
                match columns.pvalue_transform {
                    PValueTransform::None => pvalues.value(row),
                    PValueTransform::NegativeLog10 => 10f64.powf(-pvalues.value(row)),
                }
            };
            if !effect.is_finite() {
                continue;
            }
            if let Some(cutoff) = spec.per_input_p_cutoff {
                if !pvalue.is_finite() || pvalue > cutoff {
                    continue;
                }
            }
            for raw_id in raw_symbol.split(&spec.gene_delimiter) {
                let raw_id = raw_id.trim();
                if raw_id.is_empty() {
                    continue;
                }
                let symbol = mapping
                    .and_then(|mapping| mapping.get(raw_id))
                    .map(String::as_str)
                    .unwrap_or(raw_id);
                rows.push(ModalityRow {
                    symbol: symbol.to_string(),
                    effect,
                    pvalue,
                });
            }
        }
    }
    Ok(ModalityTable {
        label: label.to_string(),
        rows,
        source_rows,
    })
}

#[derive(Debug, Default, Clone, Serialize)]
struct FilterCount {
    stage: String,
    rows: u64,
}

#[derive(Debug)]
struct JoinedResult {
    input_counts: [u64; 3],
    exploded_counts: [u64; 3],
    per_input_after_cutoff: [u64; 3],
    intersection: u64,
    direction_consistent: u64,
    ranked: Vec<RankedRow>,
}

#[derive(Debug, Clone)]
struct RankedRow {
    gene_symbol: String,
    effect_rna: f64,
    pvalue_rna: f64,
    effect_tissue: f64,
    pvalue_tissue: f64,
    effect_csf: f64,
    pvalue_csf: f64,
    score: f64,
}

fn join_and_score(
    tables: &[ModalityTable],
    spec: &MultiomicConcordanceSpec,
    reporter: &NodeReporter,
) -> Result<JoinedResult, ConcordanceError> {
    if tables.len() != 3 {
        return Err(ConcordanceError::Runtime(
            "expected three modality tables".into(),
        ));
    }
    let input_counts = [
        tables[0].source_rows,
        tables[1].source_rows,
        tables[2].source_rows,
    ];
    let exploded_counts = [
        tables[0].rows.len() as u64,
        tables[1].rows.len() as u64,
        tables[2].rows.len() as u64,
    ];
    let mut index: [BTreeMap<String, (f64, f64)>; 3] =
        [BTreeMap::new(), BTreeMap::new(), BTreeMap::new()];
    for (i, table) in tables.iter().enumerate() {
        for row in &table.rows {
            index[i]
                .entry(row.symbol.clone())
                .or_insert((row.effect, row.pvalue));
        }
    }
    let per_input_after_cutoff = [
        index[0].len() as u64,
        index[1].len() as u64,
        index[2].len() as u64,
    ];

    let intersection: BTreeSet<&String> = index[0]
        .keys()
        .filter(|s| index[1].contains_key(s.as_str()) && index[2].contains_key(s.as_str()))
        .collect();
    let mut ranked = Vec::with_capacity(intersection.len());
    let mut direction_consistent: u64 = 0;
    for symbol in &intersection {
        let (e_rna, p_rna) = index[0][*symbol];
        let (e_tissue, p_tissue) = index[1][*symbol];
        let (e_csf, p_csf) = index[2][*symbol];
        let sign_rna = e_rna.signum();
        let sign_tissue = e_tissue.signum();
        let sign_csf = e_csf.signum();
        if spec.require_sign_consistency
            && !(sign_rna == sign_tissue && sign_tissue == sign_csf && sign_rna != 0.0)
        {
            continue;
        }
        direction_consistent += 1;
        let score = e_rna.abs() * e_tissue.abs() * e_csf.abs();
        ranked.push(RankedRow {
            gene_symbol: (*symbol).clone(),
            effect_rna: e_rna,
            pvalue_rna: p_rna,
            effect_tissue: e_tissue,
            pvalue_tissue: p_tissue,
            effect_csf: e_csf,
            pvalue_csf: p_csf,
            score,
        });
    }
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    reporter.info(format!(
        "multiomic_concordance: modalities={} input rows rna={} tissue={} csf={}, intersection={}, direction_consistent={}",
        tables.iter().map(|table| table.label.as_str()).collect::<Vec<_>>().join(","),
        input_counts[0], input_counts[1], input_counts[2], intersection.len(), direction_consistent
    ));

    Ok(JoinedResult {
        input_counts,
        exploded_counts,
        per_input_after_cutoff,
        intersection: intersection.len() as u64,
        direction_consistent,
        ranked,
    })
}

async fn write_outputs(
    ctx: &NodeCtx,
    prefix: &str,
    storage: &vfs::OpendalFileStorage,
    spec: &MultiomicConcordanceSpec,
    mapping_entries: usize,
    joined: &JoinedResult,
) -> Result<BTreeMap<String, dag_core::value::FileFingerprint>, ConcordanceError> {
    let mut fingerprints = BTreeMap::new();
    let mut ranked_buf = String::new();
    ranked_buf.push_str("gene_symbol\trank\tlog_fc_rna\tpvalue_rna\tlog_fc_tissue_protein\tpvalue_tissue_protein\tlog_fc_csf_protein\tpvalue_csf_protein\teffect_product_score\n");
    for (i, row) in joined.ranked.iter().enumerate() {
        ranked_buf.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            row.gene_symbol,
            i + 1,
            f64_to_str(row.effect_rna),
            f64_to_str(row.pvalue_rna),
            f64_to_str(row.effect_tissue),
            f64_to_str(row.pvalue_tissue),
            f64_to_str(row.effect_csf),
            f64_to_str(row.pvalue_csf),
            f64_to_str(row.score),
        ));
    }
    let path = format!("{prefix}/ranked_candidates.tsv");
    let fingerprint = write_vfs(ctx, storage, &path, ranked_buf.as_bytes()).await?;
    fingerprints.insert(path, fingerprint);

    let counts: Vec<FilterCount> = vec![
        FilterCount {
            stage: "input_rna".into(),
            rows: joined.input_counts[0],
        },
        FilterCount {
            stage: "input_tissue_protein".into(),
            rows: joined.input_counts[1],
        },
        FilterCount {
            stage: "input_csf_protein".into(),
            rows: joined.input_counts[2],
        },
        FilterCount {
            stage: "exploded_symbols_rna".into(),
            rows: joined.exploded_counts[0],
        },
        FilterCount {
            stage: "exploded_symbols_tissue_protein".into(),
            rows: joined.exploded_counts[1],
        },
        FilterCount {
            stage: "exploded_symbols_csf_protein".into(),
            rows: joined.exploded_counts[2],
        },
        FilterCount {
            stage: "after_cutoff_rna".into(),
            rows: joined.per_input_after_cutoff[0],
        },
        FilterCount {
            stage: "after_cutoff_tissue_protein".into(),
            rows: joined.per_input_after_cutoff[1],
        },
        FilterCount {
            stage: "after_cutoff_csf_protein".into(),
            rows: joined.per_input_after_cutoff[2],
        },
        FilterCount {
            stage: "intersection".into(),
            rows: joined.intersection,
        },
        FilterCount {
            stage: "direction_consistent".into(),
            rows: joined.direction_consistent,
        },
    ];
    let mut counts_buf = String::new();
    counts_buf.push_str("stage\trows\n");
    for c in &counts {
        counts_buf.push_str(&format!("{}\t{}\n", c.stage, c.rows));
    }
    let path = format!("{prefix}/filter_counts.tsv");
    let fingerprint = write_vfs(ctx, storage, &path, counts_buf.as_bytes()).await?;
    fingerprints.insert(path, fingerprint);

    let mut matched_buf = String::new();
    matched_buf.push_str("gene_symbol\n");
    for row in &joined.ranked {
        matched_buf.push_str(&format!("{}\n", row.gene_symbol));
    }
    let path = format!("{prefix}/matched_symbols.tsv");
    let fingerprint = write_vfs(ctx, storage, &path, matched_buf.as_bytes()).await?;
    fingerprints.insert(path, fingerprint);

    let report = serde_json::json!({
        "schema_version": "1.0",
        "node": KIND,
        "analysis": {
            "columns": {
                "rna": spec.rna,
                "tissue_protein": spec.tissue_protein,
                "csf_protein": spec.csf_protein
            },
            "gene_delimiter": spec.gene_delimiter,
            "id_mapping_entries": mapping_entries,
            "per_input_p_cutoff": spec.per_input_p_cutoff,
            "require_sign_consistency": spec.require_sign_consistency,
        },
        "filters": counts,
        "engine": {
            "package": "autonomics/multiomic_concordance",
            "version": env!("CARGO_PKG_VERSION"),
        }
    });
    let report_bytes = serde_json::to_vec_pretty(&report)
        .map_err(|e| ConcordanceError::Runtime(format!("report serialize: {e}")))?;
    let path = format!("{prefix}/run_report.json");
    let fingerprint = write_vfs(ctx, storage, &path, &report_bytes).await?;
    fingerprints.insert(path, fingerprint);
    Ok(fingerprints)
}

fn f64_to_str(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        }
    } else {
        format!("{value:.15e}")
    }
}

async fn write_vfs(
    _ctx: &NodeCtx,
    storage: &vfs::OpendalFileStorage,
    path: &str,
    bytes: &[u8],
) -> Result<dag_core::value::FileFingerprint, ConcordanceError> {
    let virtual_path = path.trim_start_matches('/');
    let op = storage.resolve(virtual_path);
    let target = storage.resolve_path(virtual_path);
    op.write(&target, bytes.to_vec())
        .await
        .map_err(|e| ConcordanceError::Runtime(format!("write `{path}`: {e}")))?;
    let digest = Sha256::digest(bytes);
    Ok(dag_core::value::FileFingerprint {
        size: bytes.len() as u64,
        mtime_ns: 0,
        content_hash: Some(format!("{digest:x}")),
        immutable_remote: true,
    })
}

fn joined_outputs(
    prefix: &str,
    fingerprints: BTreeMap<String, dag_core::value::FileFingerprint>,
) -> PortOutputs {
    let mut outputs = PortOutputs::new();
    let files = [
        "ranked_candidates.tsv",
        "filter_counts.tsv",
        "matched_symbols.tsv",
        "run_report.json",
    ];
    for (idx, name) in files.iter().enumerate() {
        let path = format!("vfs://{prefix}/{name}");
        let fingerprint = fingerprints.get(path.trim_start_matches("vfs://")).cloned();
        outputs.insert_file(
            idx as u8,
            FileRef {
                path,
                format: Some(name.trim_end_matches(".tsv").to_string()),
                fingerprint,
            },
        );
    }
    outputs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_direction_consistent_genes_by_effect_product() {
        let spec = MultiomicConcordanceSpec {
            rna: default_columns(),
            tissue_protein: default_columns(),
            csf_protein: default_columns(),
            gene_delimiter: default_gene_delimiter(),
            per_input_p_cutoff: Some(0.05),
            require_sign_consistency: true,
            artifact_prefix: "/artifacts/test".into(),
        };
        let tables = vec![
            ModalityTable {
                label: "rna".into(),
                rows: vec![
                    ModalityRow {
                        symbol: "A".into(),
                        effect: 1.0,
                        pvalue: 0.01,
                    },
                    ModalityRow {
                        symbol: "B".into(),
                        effect: -2.0,
                        pvalue: 0.01,
                    },
                    ModalityRow {
                        symbol: "C".into(),
                        effect: 1.5,
                        pvalue: 0.01,
                    },
                ],
                source_rows: 3,
            },
            ModalityTable {
                label: "tissue_protein".into(),
                rows: vec![
                    ModalityRow {
                        symbol: "A".into(),
                        effect: 0.5,
                        pvalue: 0.02,
                    },
                    ModalityRow {
                        symbol: "B".into(),
                        effect: 1.0,
                        pvalue: 0.02,
                    },
                    ModalityRow {
                        symbol: "C".into(),
                        effect: -0.4,
                        pvalue: 0.02,
                    },
                ],
                source_rows: 3,
            },
            ModalityTable {
                label: "csf_protein".into(),
                rows: vec![
                    ModalityRow {
                        symbol: "A".into(),
                        effect: 0.2,
                        pvalue: 0.03,
                    },
                    ModalityRow {
                        symbol: "B".into(),
                        effect: -1.5,
                        pvalue: 0.03,
                    },
                    ModalityRow {
                        symbol: "C".into(),
                        effect: -0.5,
                        pvalue: 0.03,
                    },
                ],
                source_rows: 3,
            },
        ];
        let reporter = NodeReporter::noop();
        let joined = join_and_score(&tables, &spec, &reporter).unwrap();
        // A: signs agree (positive); B: signs agree (negative); C: mixed (positive rna, negative tissue/csf).
        assert_eq!(joined.intersection, 3);
        // Only A has all-positive effects; B and C mix signs.
        assert_eq!(joined.direction_consistent, 1);
        assert_eq!(joined.ranked.len(), 1);
        assert_eq!(joined.ranked[0].gene_symbol, "A");
        assert!((joined.ranked[0].score - 0.1).abs() < 1e-9);
    }

    #[test]
    fn rejects_invalid_cutoff() {
        let spec = MultiomicConcordanceSpec {
            rna: default_columns(),
            tissue_protein: default_columns(),
            csf_protein: default_columns(),
            gene_delimiter: default_gene_delimiter(),
            per_input_p_cutoff: Some(1.5),
            require_sign_consistency: true,
            artifact_prefix: "relative".into(),
        };
        assert!(validate(&spec).is_err());
    }
}
