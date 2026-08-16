//! Generic over-representation analysis (ORA) node.
//!
//! The node treats gene-set annotations as data ports and does not depend on a
//! particular annotation source. This makes the same node usable for KEGG, GO,
//! Reactome, MSigDB, or custom long-format gene-set tables.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray};
use arrow_cast::cast as cast_array;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, node_event::NodeReporter};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use hypothesize as h;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use statrs::distribution::{DiscreteCDF, Hypergeometric};

const ENRICHMENT_ORA_KIND: &str = "enrichment_ora";

fn default_gene_col() -> String {
    "gene_id".to_string()
}

fn default_min_set_size() -> usize {
    5
}

fn default_max_set_size() -> usize {
    500
}

fn default_min_hits() -> usize {
    2
}

fn default_exclude_sets() -> Vec<String> {
    vec![
        "map01100".to_string(),
        "map01110".to_string(),
        "map01120".to_string(),
    ]
}

fn default_background_mode() -> String {
    "annotation_all".to_string()
}

fn default_alternative() -> String {
    "greater".to_string()
}

fn default_test() -> String {
    "hypergeometric".to_string()
}

fn default_correction() -> String {
    "BH".to_string()
}

fn default_alpha() -> f64 {
    0.05
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EnrichmentOraSpec {
    #[serde(default = "default_gene_col")]
    pub gene_col: String,
    #[serde(default = "default_min_set_size")]
    pub min_set_size: usize,
    #[serde(default = "default_max_set_size")]
    pub max_set_size: usize,
    #[serde(default = "default_min_hits")]
    pub min_hits: usize,
    #[serde(default = "default_exclude_sets")]
    pub exclude_sets: Vec<String>,
    #[serde(default = "default_background_mode")]
    pub background_mode: String,
    #[serde(default = "default_alternative")]
    pub alternative: String,
    #[serde(default = "default_test")]
    pub test: String,
    #[serde(default = "default_correction")]
    pub correction: String,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
}

pub struct EnrichmentOraNodeFactory;

impl NodeFactory for EnrichmentOraNodeFactory {
    fn kind(&self) -> &'static str {
        ENRICHMENT_ORA_KIND
    }

    fn desc(&self) -> &'static str {
        "Generic gene-set over-representation analysis (ORA)."
    }

    fn doc(&self) -> &'static str {
        "Input port 0 is a query gene table, port 1 is a gene_id/set_id mapping, \
        port 2 is set metadata, and optional port 3 is a background gene table. \
        Performs exact hypergeometric/Fisher or chi-square enrichment tests, \
        applies multiple-testing correction, and reports hit genes. Background \
        mode is annotation_all, given, or detected."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnrichmentOraSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_input_port(None)
            .add_input_port(None)
            .add_input_port(None)
            .add_output_port(Some(output_schema()))
            // Port 3 is optional. Required ports are checked at execution time.
            .set_fixed_input(false)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EnrichmentOraSpec = serde_json::from_value(spec)?;
        validate_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(Box::new(EnrichmentOraNode {
            meta: Self.ports(),
            spec,
        }))
    }
}

#[derive(Clone)]
struct EnrichmentOraNode {
    meta: NodePorts,
    spec: EnrichmentOraSpec,
}

#[derive(Debug, Clone)]
struct SetMetadata {
    set_name: String,
    set_class: Option<String>,
}

#[derive(Debug)]
struct EnrichmentResult {
    set_id: String,
    set_name: String,
    set_class: Option<String>,
    hit_n: i64,
    set_n: i64,
    expected_hits: f64,
    fold_enrichment: f64,
    p_raw: f64,
    p_adj: f64,
    hit_genes: String,
}

impl EnrichmentOraNode {
    #[cfg(test)]
    pub fn new(spec: EnrichmentOraSpec) -> Self {
        Self {
            meta: EnrichmentOraNodeFactory.ports(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for EnrichmentOraNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        ENRICHMENT_ORA_KIND
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
        let mut connected_ports = BTreeSet::new();
        for input in inputs {
            if input.port > 3 {
                return Err(node_error(format!(
                    "input port {} does not exist; valid ports are 0-3",
                    input.port
                )));
            }
            if !connected_ports.insert(input.port) {
                return Err(node_error(format!(
                    "input port {} is connected more than once",
                    input.port
                )));
            }
        }

        let ports = collect_ports(inputs).await?;
        for port in 0..3 {
            if !ports.contains_key(&port) {
                return Err(node_error(format!("input port {port} is required")));
            }
        }

        let query = extract_unique_column(&ports, 0, &self.spec.gene_col)?;
        let annotation = extract_annotation(&ports)?;
        let metadata = extract_metadata(&ports)?;
        let background = if ports.contains_key(&3) {
            extract_first_column(&ports, 3)?
        } else {
            BTreeSet::new()
        };

        if query.is_empty() {
            return Err(node_error("query gene input contains no non-null IDs"));
        }
        if annotation.is_empty() {
            return Err(node_error("annotation mapping contains no rows"));
        }

        let annotation_genes: BTreeSet<_> = annotation.iter().map(|row| row.0.clone()).collect();
        let mapped_query_n = query.intersection(&annotation_genes).count();
        let mapping_rate = mapped_query_n as f64 / query.len() as f64;
        if mapping_rate < 0.8 {
            reporter.warn(format!(
                "only {:.1}% of query genes occur in the annotation mapping",
                mapping_rate * 100.0
            ));
        }

        let universe = match self.spec.background_mode.as_str() {
            "annotation_all" => annotation_genes,
            "given" => {
                if !ports.contains_key(&3) {
                    return Err(node_error("background_mode='given' requires input port 3"));
                }
                background
            }
            "detected" => {
                reporter.warn(
                    "background_mode='detected' uses the distinct IDs in port 0; \
                     port 0 must contain the full detected table for this mode",
                );
                query.clone()
            }
            _ => unreachable!("background mode validated at build time"),
        };

        if universe.is_empty() {
            return Err(node_error("background universe is empty"));
        }
        if universe.len() < 500 {
            reporter.warn(format!(
                "background universe is small: {} genes (<500)",
                universe.len()
            ));
        }

        let effective_query: BTreeSet<String> = query.intersection(&universe).cloned().collect();
        let query_n = effective_query.len();
        let universe_n = universe.len();
        if query_n == 0 {
            return Err(node_error(
                "no query genes are present in the background universe",
            ));
        }
        if query_n * 5 > universe_n {
            reporter.warn(format!(
                "query set is more than 20% of the background: {query_n}/{universe_n} genes"
            ));
        }
        if query_n > universe_n {
            return Err(node_error(
                "query set is larger than the background universe",
            ));
        }

        let mut sets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (gene_id, set_id) in annotation {
            if universe.contains(&gene_id) {
                sets.entry(set_id).or_default().insert(gene_id);
            }
        }

        let excluded: BTreeSet<_> = self.spec.exclude_sets.iter().cloned().collect();
        let mut candidates = Vec::new();
        for (set_id, set_genes) in sets {
            if excluded.contains(&set_id) {
                continue;
            }
            let set_n = set_genes.len();
            if set_n < self.spec.min_set_size {
                continue;
            }
            if set_n > self.spec.max_set_size {
                continue;
            }

            let hits: Vec<_> = set_genes.intersection(&effective_query).cloned().collect();
            let hit_n = hits.len();
            if hit_n < self.spec.min_hits {
                continue;
            }
            let meta = metadata.get(&set_id).ok_or_else(|| {
                node_error(format!(
                    "set '{set_id}' occurs in the annotation mapping but not in metadata"
                ))
            })?;

            let set_n_f = set_n as f64;
            let query_n_f = query_n as f64;
            let universe_n_f = universe_n as f64;
            let expected_hits = set_n_f * query_n_f / universe_n_f;
            let fold_enrichment = (hit_n as f64 / set_n_f) / (query_n_f / universe_n_f);
            let p_raw = raw_p_value(
                &self.spec.test,
                &self.spec.alternative,
                universe_n,
                set_n,
                query_n,
                hit_n,
            )?;

            candidates.push(EnrichmentResult {
                set_id,
                set_name: meta.set_name.clone(),
                set_class: meta.set_class.clone(),
                hit_n: hit_n as i64,
                set_n: set_n as i64,
                expected_hits,
                fold_enrichment,
                p_raw,
                p_adj: 0.0,
                hit_genes: hits.join(","),
            });
        }

        let method = h::AdjustMethod::parse(&self.spec.correction)
            .map_err(|e| node_error(format!("invalid correction method: {e}")))?;
        let p_raw: Vec<_> = candidates.iter().map(|row| row.p_raw).collect();
        let p_adj = h::p_adjust_raw(&p_raw, method, Some(candidates.len()))
            .map_err(|e| node_error(format!("p-value adjustment failed: {e}")))?;
        for (row, adjusted) in candidates.iter_mut().zip(p_adj) {
            row.p_adj = adjusted;
        }
        candidates.sort_by(|a, b| {
            a.p_adj
                .partial_cmp(&b.p_adj)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.set_id.cmp(&b.set_id))
        });

        let batch = build_output_batch(
            &candidates,
            universe_n as i64,
            query_n as i64,
            self.spec.alpha,
        )?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| node_error(format!("failed to create output DataFrame: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

fn validate_spec(spec: &EnrichmentOraSpec) -> Result<(), String> {
    if spec.gene_col.trim().is_empty() {
        return Err("gene_col must not be empty".to_string());
    }
    if spec.min_set_size > spec.max_set_size {
        return Err("min_set_size must be less than or equal to max_set_size".to_string());
    }
    if !(0.0..=1.0).contains(&spec.alpha) || spec.alpha == 0.0 {
        return Err("alpha must be in (0, 1]".to_string());
    }
    if !matches!(
        spec.background_mode.as_str(),
        "annotation_all" | "given" | "detected"
    ) {
        return Err(format!(
            "unknown background_mode '{}' (expected annotation_all|given|detected)",
            spec.background_mode
        ));
    }
    if !matches!(
        spec.alternative.as_str(),
        "greater" | "two-sided" | "two_sided"
    ) {
        return Err(format!(
            "unknown alternative '{}' (expected greater|two-sided)",
            spec.alternative
        ));
    }
    if !matches!(spec.test.as_str(), "hypergeometric" | "fisher" | "chi2") {
        return Err(format!(
            "unknown test '{}' (expected hypergeometric|fisher|chi2)",
            spec.test
        ));
    }
    if spec.test == "chi2" && !spec.alternative.starts_with("two") {
        return Err("chi2 supports only the two-sided alternative".to_string());
    }
    h::AdjustMethod::parse(&spec.correction)
        .map(|_| ())
        .map_err(|e| format!("invalid correction method: {e}"))
}

fn raw_p_value(
    test: &str,
    alternative: &str,
    universe_n: usize,
    set_n: usize,
    query_n: usize,
    hit_n: usize,
) -> Result<f64, DagError> {
    let n = universe_n as u64;
    let k = set_n as u64;
    let draws = query_n as u64;
    let observed = hit_n as u64;

    if k > n || draws > n {
        return Err(node_error(
            "gene-set size or query size is larger than the background universe",
        ));
    }

    if test == "chi2" {
        let non_set_query = (query_n - hit_n) as f64;
        let set_non_query = (set_n - hit_n) as f64;
        let outside = (universe_n - set_n - query_n + hit_n) as f64;
        let denominator = set_n as f64
            * (universe_n - set_n) as f64
            * query_n as f64
            * (universe_n - query_n) as f64;
        if denominator == 0.0 {
            return Ok(1.0);
        }
        let statistic = universe_n as f64
            * (hit_n as f64 * outside - non_set_query * set_non_query).powi(2)
            / denominator;
        return Ok(h::chisq_sf(statistic, 1.0).clamp(0.0, 1.0));
    }

    if test == "fisher" {
        let table = [
            [hit_n as u32, (set_n - hit_n) as u32],
            [
                (query_n - hit_n) as u32,
                (universe_n - set_n - query_n + hit_n) as u32,
            ],
        ];
        let alternative = if alternative.starts_with("two") {
            h::Alternative::TwoSided
        } else {
            h::Alternative::Greater
        };
        return h::fisher_exact(&table, alternative, 0.95)
            .map(|result| result.p_value)
            .map_err(|e| node_error(format!("Fisher exact p-value failed: {e}")));
    }

    let dist = Hypergeometric::new(n, k, draws)
        .map_err(|e| node_error(format!("invalid hypergeometric parameters: {e}")))?;
    if alternative.starts_with("two") {
        let table = [
            [hit_n as u32, (query_n - hit_n) as u32],
            [
                (set_n - hit_n) as u32,
                (universe_n - set_n - query_n + hit_n) as u32,
            ],
        ];
        h::fisher_exact(&table, h::Alternative::TwoSided, 0.95)
            .map(|result| result.p_value)
            .map_err(|e| node_error(format!("two-sided exact p-value failed: {e}")))
    } else {
        if observed == 0 {
            return Ok(1.0);
        }
        Ok(dist.sf(observed - 1).clamp(0.0, 1.0))
    }
}

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("set_id", DataType::Utf8, false),
        Field::new("set_name", DataType::Utf8, false),
        Field::new("set_class", DataType::Utf8, true),
        Field::new("universe_n", DataType::Int64, false),
        Field::new("query_n", DataType::Int64, false),
        Field::new("hit_n", DataType::Int64, false),
        Field::new("set_n", DataType::Int64, false),
        Field::new("expected_hits", DataType::Float64, false),
        Field::new("fold_enrichment", DataType::Float64, false),
        Field::new("p_raw", DataType::Float64, false),
        Field::new("p_adj", DataType::Float64, false),
        Field::new("reject", DataType::Int32, false),
        Field::new("hit_genes", DataType::Utf8, false),
    ]))
}

fn build_output_batch(
    results: &[EnrichmentResult],
    universe_n: i64,
    query_n: i64,
    alpha: f64,
) -> Result<RecordBatch, DagError> {
    let set_ids: Vec<&str> = results.iter().map(|row| row.set_id.as_str()).collect();
    let set_names: Vec<&str> = results.iter().map(|row| row.set_name.as_str()).collect();
    let set_classes: Vec<Option<&str>> =
        results.iter().map(|row| row.set_class.as_deref()).collect();
    let hit_ns: Vec<i64> = results.iter().map(|row| row.hit_n).collect();
    let set_ns: Vec<i64> = results.iter().map(|row| row.set_n).collect();
    let expected: Vec<f64> = results.iter().map(|row| row.expected_hits).collect();
    let fold: Vec<f64> = results.iter().map(|row| row.fold_enrichment).collect();
    let p_raw: Vec<f64> = results.iter().map(|row| row.p_raw).collect();
    let p_adj: Vec<f64> = results.iter().map(|row| row.p_adj).collect();
    let reject: Vec<i32> = p_adj.iter().map(|&p| i32::from(p < alpha)).collect();
    let hit_genes: Vec<&str> = results.iter().map(|row| row.hit_genes.as_str()).collect();

    RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(StringArray::from(set_ids)),
            Arc::new(StringArray::from(set_names)),
            Arc::new(StringArray::from(set_classes)),
            Arc::new(Int64Array::from(vec![universe_n; results.len()])),
            Arc::new(Int64Array::from(vec![query_n; results.len()])),
            Arc::new(Int64Array::from(hit_ns)),
            Arc::new(Int64Array::from(set_ns)),
            Arc::new(Float64Array::from(expected)),
            Arc::new(Float64Array::from(fold)),
            Arc::new(Float64Array::from(p_raw)),
            Arc::new(Float64Array::from(p_adj)),
            Arc::new(Int32Array::from(reject)),
            Arc::new(StringArray::from(hit_genes)),
        ],
    )
    .map_err(|e| node_error(format!("failed to build output batch: {e}")))
}

fn node_error(message: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: ENRICHMENT_ORA_KIND.to_string(),
        msg: message.into(),
    }
}

async fn collect_ports(inputs: &[NodeInput]) -> Result<BTreeMap<u8, Vec<RecordBatch>>, DagError> {
    let mut ports = BTreeMap::new();
    for input in inputs {
        let collected =
            input.data.clone().collect().await.map_err(|e| {
                node_error(format!("failed to collect input port {}: {e}", input.port))
            })?;
        ports
            .entry(input.port)
            .or_insert_with(Vec::new)
            .extend(collected);
    }
    Ok(ports)
}

fn required_port(
    ports: &BTreeMap<u8, Vec<RecordBatch>>,
    port: u8,
) -> Result<&Vec<RecordBatch>, DagError> {
    ports
        .get(&port)
        .ok_or_else(|| node_error(format!("input port {port} is not connected")))
}

fn extract_unique_column(
    ports: &BTreeMap<u8, Vec<RecordBatch>>,
    port: u8,
    column: &str,
) -> Result<BTreeSet<String>, DagError> {
    let batches = required_port(ports, port)?;
    let mut values = BTreeSet::new();
    for batch in batches {
        let array = batch
            .column_by_name(column)
            .ok_or_else(|| node_error(format!("input port {port} is missing column '{column}'")))?;
        for value in string_values(array.as_ref())?.into_iter().flatten() {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                values.insert(trimmed.to_string());
            }
        }
    }
    Ok(values)
}

fn extract_first_column(
    ports: &BTreeMap<u8, Vec<RecordBatch>>,
    port: u8,
) -> Result<BTreeSet<String>, DagError> {
    let batches = required_port(ports, port)?;
    let mut values = BTreeSet::new();
    for batch in batches {
        if batch.num_columns() == 0 {
            return Err(node_error(format!("input port {port} has no columns")));
        }
        for value in string_values(batch.column(0).as_ref())?
            .into_iter()
            .flatten()
        {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                values.insert(trimmed.to_string());
            }
        }
    }
    Ok(values)
}

fn extract_annotation(
    ports: &BTreeMap<u8, Vec<RecordBatch>>,
) -> Result<Vec<(String, String)>, DagError> {
    let batches = required_port(ports, 1)?;
    let mut rows = Vec::new();
    for batch in batches {
        let genes = required_string_column(batch, 1, "gene_id")?;
        let sets = required_string_column(batch, 1, "set_id")?;
        for (gene, set) in genes.into_iter().zip(sets) {
            if let (Some(gene), Some(set)) = (gene, set) {
                let gene = gene.trim();
                let set = set.trim();
                if !gene.is_empty() && !set.is_empty() {
                    rows.push((gene.to_string(), set.to_string()));
                }
            }
        }
    }
    Ok(rows)
}

fn extract_metadata(
    ports: &BTreeMap<u8, Vec<RecordBatch>>,
) -> Result<BTreeMap<String, SetMetadata>, DagError> {
    let batches = required_port(ports, 2)?;
    let mut metadata = BTreeMap::new();
    for batch in batches {
        let set_ids = required_string_column(batch, 2, "set_id")?;
        let names = required_string_column(batch, 2, "set_name")?;
        let classes = optional_string_column(batch, "set_class")?;
        for (i, (set_id, set_name)) in set_ids.into_iter().zip(names).enumerate() {
            let Some(set_id) = non_empty(set_id) else {
                continue;
            };
            let set_name = non_empty(set_name).ok_or_else(|| {
                node_error(format!("metadata set '{set_id}' has an empty set_name"))
            })?;
            metadata.entry(set_id).or_insert(SetMetadata {
                set_name,
                set_class: classes.as_ref().and_then(|values| {
                    values
                        .get(i)
                        .cloned()
                        .flatten()
                        .filter(|v| !v.trim().is_empty())
                }),
            });
        }
    }
    Ok(metadata)
}

fn required_string_column(
    batch: &RecordBatch,
    port: u8,
    column: &str,
) -> Result<Vec<Option<String>>, DagError> {
    batch
        .column_by_name(column)
        .map(|array| string_values(array.as_ref()))
        .transpose()?
        .ok_or_else(|| node_error(format!("input port {port} is missing column '{column}'")))
}

fn optional_string_column(
    batch: &RecordBatch,
    column: &str,
) -> Result<Option<Vec<Option<String>>>, DagError> {
    batch
        .column_by_name(column)
        .map(|array| string_values(array.as_ref()))
        .transpose()
}

fn non_empty(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn string_values(array: &dyn Array) -> Result<Vec<Option<String>>, DagError> {
    let array = cast_array(array, &DataType::Utf8)
        .map_err(|e| node_error(format!("failed to normalize an ID column to Utf8: {e}")))?;
    let array = array
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| node_error("ID column could not be normalized to Utf8"))?;
    Ok((0..array.len())
        .map(|i| (!array.is_null(i)).then(|| array.value(i).to_string()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::ArrayRef;
    use datafusion::prelude::SessionContext;

    fn node_ctx() -> NodeCtx {
        NodeCtx {
            runtime_env: SessionContext::new().runtime_env(),
            opendal: None,
            global_sem: None,
        }
    }

    fn query_batch(numeric: bool) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "gene_id",
            if numeric {
                DataType::Int64
            } else {
                DataType::Utf8
            },
            false,
        )]));
        let genes: ArrayRef = if numeric {
            Arc::new(arrow_array::Int64Array::from(vec![1, 2, 3, 4]))
        } else {
            Arc::new(StringArray::from(vec!["A", "B", "C", "D"]))
        };
        RecordBatch::try_new(schema, vec![genes]).unwrap()
    }

    fn mapping_batch(gene_type: DataType) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("gene_id", gene_type.clone(), false),
            Field::new("set_id", DataType::Utf8, false),
        ]));
        let genes: ArrayRef = if gene_type == DataType::Int64 {
            Arc::new(arrow_array::Int64Array::from(vec![
                1, 2, 3, 4, 5, 1, 2, 6, 7, 8, 9, 10, 8, 9,
            ]))
        } else {
            Arc::new(StringArray::from(vec![
                "A", "B", "C", "D", "E", "A", "B", "F", "G", "J", "J", "J", "H", "I",
            ]))
        };
        let sets: ArrayRef = Arc::new(StringArray::from(vec![
            "S1", "S1", "S1", "S1", "S1", "S2", "S2", "S2", "S2", "GLOBAL", "TINY", "OFF", "OFF",
            "OFF",
        ]));
        RecordBatch::try_new(schema, vec![genes, sets]).unwrap()
    }

    fn metadata_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("set_id", DataType::Utf8, false),
            Field::new("set_name", DataType::Utf8, false),
            Field::new("set_class", DataType::Utf8, true),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["S1", "S2", "GLOBAL", "TINY", "OFF"])),
                Arc::new(StringArray::from(vec![
                    "Set one",
                    "Set two",
                    "Global",
                    "Tiny",
                    "Off-universe",
                ])),
                Arc::new(StringArray::from(vec![
                    Some("class-a"),
                    Some("class-b"),
                    None,
                    None,
                    None,
                ])),
            ],
        )
        .unwrap()
    }

    fn background_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "gene_id",
            DataType::Utf8,
            false,
        )]));
        RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![
                "A", "B", "C", "D", "E", "F", "G", "H", "I",
            ])) as ArrayRef],
        )
        .unwrap()
    }

    fn inputs(ctx: &SessionContext, with_background: bool, numeric_ids: bool) -> Vec<NodeInput> {
        let mut inputs = vec![
            NodeInput {
                port: 0,
                data: ctx.read_batch(query_batch(numeric_ids)).unwrap(),
            },
            NodeInput {
                port: 1,
                data: ctx
                    .read_batch(mapping_batch(if numeric_ids {
                        DataType::Int64
                    } else {
                        DataType::Utf8
                    }))
                    .unwrap(),
            },
            NodeInput {
                port: 2,
                data: ctx.read_batch(metadata_batch()).unwrap(),
            },
        ];
        if with_background {
            inputs.push(NodeInput {
                port: 3,
                data: ctx.read_batch(background_batch()).unwrap(),
            });
        }
        inputs
    }

    fn spec(background_mode: &str) -> EnrichmentOraSpec {
        EnrichmentOraSpec {
            gene_col: default_gene_col(),
            min_set_size: 2,
            max_set_size: 10,
            min_hits: 2,
            exclude_sets: vec!["GLOBAL".to_string()],
            background_mode: background_mode.to_string(),
            alternative: "greater".to_string(),
            test: "hypergeometric".to_string(),
            correction: "BH".to_string(),
            alpha: 0.05,
        }
    }

    #[test]
    fn hypergeometric_p_value_is_exact() {
        let p = raw_p_value("hypergeometric", "greater", 10, 5, 4, 4).unwrap();
        assert!((p - 1.0 / 42.0).abs() < 1e-14, "p={p}");
    }

    #[test]
    fn fisher_test_matches_hypergeometric_tail() {
        for (universe_n, set_n, query_n, hit_n) in [(10, 5, 4, 4), (20, 7, 6, 3), (100, 20, 15, 5)]
        {
            let hyper = raw_p_value(
                "hypergeometric",
                "greater",
                universe_n,
                set_n,
                query_n,
                hit_n,
            )
            .unwrap();
            let fisher =
                raw_p_value("fisher", "greater", universe_n, set_n, query_n, hit_n).unwrap();
            assert!((hyper - fisher).abs() <= hyper.abs() * 1e-12);
        }
    }

    #[tokio::test]
    async fn executes_generic_ora_and_reports_hit_genes() {
        let upstream = SessionContext::new();
        let mut node = EnrichmentOraNode::new(spec("annotation_all"));
        let outputs = node
            .execute(
                &node_ctx(),
                &inputs(&upstream, false, false),
                &NodeReporter::noop(),
            )
            .await
            .unwrap();
        let batches = outputs.get(&0).unwrap().clone().collect().await.unwrap();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema(), output_schema());

        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let hits = batch
            .column(5)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let set_sizes = batch
            .column(6)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let p_raw = batch
            .column(9)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let hit_genes = batch
            .column(12)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();

        assert_eq!(ids.value(0), "S1");
        assert_eq!(hits.value(0), 4);
        assert_eq!(set_sizes.value(0), 5);
        assert!((p_raw.value(0) - 1.0 / 42.0).abs() < 1e-14);
        assert_eq!(hit_genes.value(0), "A,B,C,D");
    }

    #[tokio::test]
    async fn given_background_changes_universe_and_counts() {
        let upstream = SessionContext::new();
        let mut node = EnrichmentOraNode::new(spec("given"));
        let outputs = node
            .execute(
                &node_ctx(),
                &inputs(&upstream, true, false),
                &NodeReporter::noop(),
            )
            .await
            .unwrap();
        let batches = outputs.get(&0).unwrap().clone().collect().await.unwrap();
        let universe = batches[0]
            .column(3)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(universe.value(0), 9);
    }

    #[tokio::test]
    async fn numeric_entrez_ids_are_normalized() {
        let upstream = SessionContext::new();
        let mut node = EnrichmentOraNode::new(spec("annotation_all"));
        let outputs = node
            .execute(
                &node_ctx(),
                &inputs(&upstream, false, true),
                &NodeReporter::noop(),
            )
            .await
            .unwrap();
        let batches = outputs.get(&0).unwrap().clone().collect().await.unwrap();
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let hit_genes = batches[0]
            .column(12)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(ids.value(0), "S1");
        assert_eq!(hit_genes.value(0), "1,2,3,4");
    }
}
