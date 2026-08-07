//! OpenGWAS source nodes — table-fetching endpoints as DAG sources.
//!
//! Seven source nodes that pull tabular data from the [OpenGWAS](
//! https://gwas-api.mrcieu.ac.uk/) REST API and emit it as DataFusion
//! `DataFrame`s, so GWAS association tables, variant annotations, study
//! metadata, etc. can flow through a DAG (join, filter via `sql_node`,
//! sink to CSV/Iceberg, …).
//!
//! All are zero-input / single-output source nodes. They reuse the
//! `opengwas` SDK client and require the `OPENGWAS_TOKEN` environment
//! variable.
//!
//! # Nodes
//!
//! | Kind | Endpoint | Output |
//! |------|----------|--------|
//! | `source_opengwas_associations` | `/associations` | variant × study associations |
//! | `source_opengwas_phewas` | `/phewas` | variant × trait PheWAS hits |
//! | `source_opengwas_gwasinfo` | `/gwasinfo` | study metadata |
//! | `source_opengwas_gwasinfo_search` | `/gwasinfo` (SQL search) | matching study metadata |
//! | `source_opengwas_variants_rsid` | `/variants/rsid` | variant annotations by rsID |
//! | `source_opengwas_variants_chrpos` | `/variants/chrpos` | variant annotations by chr:pos |
//! | `source_opengwas_ld_clump` | `/ld/clump` | clumped independent loci |

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::common::HashMap;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::node::{DagNode, NodePorts};

use opengwas::types::*;

// ===========================================================================
// Shared JSON → Arrow helpers (also used by source_opengwas_tophits)
// ===========================================================================

/// GWAS field ordering priority — common columns appear first in the output.
pub(crate) const FIELD_PRIORITY: &[&str] = &[
    "rsid",
    "variant",
    "chr",
    "chromosome",
    "position",
    "pos",
    "ea",
    "nea",
    "eaf",
    "beta",
    "se",
    "pval",
    "p",
    "nsnp",
    "trait",
    "study_id",
    "id",
    "samplesize",
    "sample_size",
    "ncase",
    "ncontrol",
    "unit",
    "population",
];

/// Extract a flat row array from an API response that may be a bare JSON
/// array, or an object keyed by study ID whose values are arrays.
pub(crate) fn extract_rows(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(arr) => arr.clone(),
        Value::Object(map) => {
            let all_arrays = !map.is_empty() && map.values().all(|v| v.is_array());
            if all_arrays {
                map.values()
                    .flat_map(|v| v.as_array().into_iter().flatten().cloned())
                    .collect()
            } else {
                vec![value.clone()]
            }
        }
        _ => vec![value.clone()],
    }
}

/// Infer the column set from the union of keys across all rows, ordered by
/// [`FIELD_PRIORITY`] then alphabetical for remaining keys.
pub(crate) fn infer_columns(rows: &[Value]) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if seen.insert(key.clone()) {
                    columns.push(key.clone());
                }
            }
        }
    }
    columns.sort_by(|a, b| {
        let ai = FIELD_PRIORITY.iter().position(|&p| p == a).unwrap_or(999);
        let bi = FIELD_PRIORITY.iter().position(|&p| p == b).unwrap_or(999);
        ai.cmp(&bi).then_with(|| a.cmp(b))
    });
    columns
}

/// Infer the Arrow [`DataType`] for a column by scanning all non-null values.
pub(crate) fn infer_type(rows: &[Value], key: &str) -> DataType {
    let mut all_int = true;
    let mut all_num = true;
    let mut any_non_null = false;
    for row in rows {
        match row.get(key) {
            Some(Value::Number(n)) => {
                any_non_null = true;
                if n.is_i64() || n.is_u64() {
                    // integer
                } else {
                    all_int = false;
                }
            }
            Some(Value::String(_)) => {
                any_non_null = true;
                all_int = false;
                all_num = false;
            }
            Some(Value::Bool(_)) => {
                any_non_null = true;
                all_int = false;
                all_num = false;
            }
            Some(Value::Null) | None => {}
            Some(_) => {
                any_non_null = true;
                all_int = false;
                all_num = false;
            }
        }
    }
    if !any_non_null {
        DataType::Utf8
    } else if all_int {
        DataType::Int64
    } else if all_num {
        DataType::Float64
    } else {
        DataType::Utf8
    }
}

/// Build an Arrow [`RecordBatch`] from a JSON row array, inferring the schema
/// dynamically. Used by all JSON-returning OpenGWAS endpoints.
pub(crate) fn build_json_batch(rows: &[Value]) -> Result<RecordBatch, DagError> {
    if rows.is_empty() {
        return Err(DagError::Schedule(
            "OpenGWAS endpoint returned no rows".into(),
        ));
    }

    let columns = infer_columns(rows);
    let fields: Vec<Field> = columns
        .iter()
        .map(|name| Field::new(name, infer_type(rows, name), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(columns.len());
    for col in &columns {
        let dtype = schema.field_with_name(col).unwrap().data_type().clone();
        let arr = build_column(rows, col, &dtype);
        arrays.push(arr);
    }

    RecordBatch::try_new(schema, arrays)
        .map_err(|e| DagError::Schedule(format!("failed to build OpenGWAS batch: {e}")))
}

/// Build a single column array from JSON values.
pub(crate) fn build_column(rows: &[Value], key: &str, dtype: &DataType) -> Arc<dyn Array> {
    match dtype {
        DataType::Int64 => {
            let vals: Vec<Option<i64>> = rows
                .iter()
                .map(|r| r.get(key).and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64))))
                .collect();
            Arc::new(Int64Array::from(vals))
        }
        DataType::Float64 => {
            let vals: Vec<Option<f64>> = rows
                .iter()
                .map(|r| r.get(key).and_then(|v| v.as_f64()))
                .collect();
            Arc::new(Float64Array::from(vals))
        }
        _ => {
            let vals: Vec<Option<String>> = rows
                .iter()
                .map(|r| match r.get(key) {
                    Some(Value::Null) | None => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Number(n)) => Some(n.to_string()),
                    Some(Value::Bool(b)) => Some(b.to_string()),
                    Some(other) => Some(other.to_string()),
                })
                .collect();
            let refs: Vec<Option<&str>> = vals.iter().map(|o| o.as_deref()).collect();
            Arc::new(StringArray::from(refs))
        }
    }
}

/// Create an OpenGWAS client from the environment.
pub(crate) fn make_client() -> Result<opengwas::OpengwasClient, DagError> {
    opengwas::OpengwasClient::new(None).map_err(|e| {
        DagError::Schedule(format!(
            "failed to create OpenGWAS client (is OPENGWAS_TOKEN set?): {e}"
        ))
    })
}

/// Convert a JSON response into a single-output [`PortOutputs`] via dynamic
/// schema inference. Shared by all JSON-returning source nodes.
async fn json_to_output(
    ctx: &NodeCtx,
    value: &Value,
    endpoint: &str,
) -> Result<PortOutputs, DagError> {
    let rows = extract_rows(value);
    tracing::info!("OpenGWAS {}: {} rows returned", endpoint, rows.len());
    let batch = build_json_batch(&rows)?;
    let session = ctx.session();
    let df = session
        .read_batch(batch)
        .map_err(|e| DagError::Schedule(format!("failed to read OpenGWAS batch: {e}")))?;
    let mut res: PortOutputs = HashMap::new();
    res.insert(0, df);
    Ok(res)
}

pub(crate) fn single_output_port() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

// ===========================================================================
// GwasInfo → Arrow (typed conversion for gwasinfo / gwasinfo_search)
// ===========================================================================

/// Build an Arrow batch from a list of [`GwasInfo`] records.
///
/// Every field of `GwasInfo` becomes a column; string fields → `Utf8`,
/// integer fields → `Int64`, `sd` → `Float64`.
fn build_gwasinfo_batch(rows: &[GwasInfo]) -> Result<RecordBatch, DagError> {
    if rows.is_empty() {
        return Err(DagError::Schedule(
            "OpenGWAS /gwasinfo returned no records".into(),
        ));
    }

    macro_rules! opt_str_col {
        ($field:ident) => {{
            let vals: Vec<Option<String>> = rows.iter().map(|r| r.$field.clone()).collect();
            let refs: Vec<Option<&str>> = vals.iter().map(|o| o.as_deref()).collect();
            Arc::new(StringArray::from(refs)) as Arc<dyn Array>
        }};
    }
    macro_rules! opt_i64_col {
        ($field:ident) => {{
            Arc::new(Int64Array::from(
                rows.iter().map(|r| r.$field).collect::<Vec<_>>(),
            )) as Arc<dyn Array>
        }};
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, true),
        Field::new("trait", DataType::Utf8, true),
        Field::new("group_name", DataType::Utf8, true),
        Field::new("category", DataType::Utf8, true),
        Field::new("subcategory", DataType::Utf8, true),
        Field::new("population", DataType::Utf8, true),
        Field::new("sex", DataType::Utf8, true),
        Field::new("author", DataType::Utf8, true),
        Field::new("year", DataType::Int64, true),
        Field::new("pmid", DataType::Int64, true),
        Field::new("nsnp", DataType::Int64, true),
        Field::new("sample_size", DataType::Int64, true),
        Field::new("ncase", DataType::Int64, true),
        Field::new("ncontrol", DataType::Int64, true),
        Field::new("mr", DataType::Int64, true),
        Field::new("priority", DataType::Int64, true),
        Field::new("is_nc", DataType::Int64, true),
        Field::new("sd", DataType::Float64, true),
        Field::new("unit", DataType::Utf8, true),
        Field::new("build", DataType::Utf8, true),
        Field::new("ontology", DataType::Utf8, true),
        Field::new("consortium", DataType::Utf8, true),
        Field::new("doi", DataType::Utf8, true),
        Field::new("study_design", DataType::Utf8, true),
        Field::new("covariates", DataType::Utf8, true),
        Field::new("coverage", DataType::Utf8, true),
        Field::new("qc_prior_to_upload", DataType::Utf8, true),
        Field::new("imputation_panel", DataType::Utf8, true),
        Field::new("beta_transformation", DataType::Utf8, true),
        Field::new("note", DataType::Utf8, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            opt_str_col!(id),
            opt_str_col!(trait_),
            opt_str_col!(group_name),
            opt_str_col!(category),
            opt_str_col!(subcategory),
            opt_str_col!(population),
            opt_str_col!(sex),
            opt_str_col!(author),
            opt_i64_col!(year),
            opt_i64_col!(pmid),
            opt_i64_col!(nsnp),
            opt_i64_col!(sample_size),
            opt_i64_col!(ncase),
            opt_i64_col!(ncontrol),
            opt_i64_col!(mr),
            opt_i64_col!(priority),
            opt_i64_col!(is_nc),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.sd).collect::<Vec<_>>(),
            )) as Arc<dyn Array>,
            opt_str_col!(unit),
            opt_str_col!(build),
            opt_str_col!(ontology),
            opt_str_col!(consortium),
            opt_str_col!(doi),
            opt_str_col!(study_design),
            opt_str_col!(covariates),
            opt_str_col!(coverage),
            opt_str_col!(qc_prior_to_upload),
            opt_str_col!(imputation_panel),
            opt_str_col!(beta_transformation),
            opt_str_col!(note),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build gwasinfo batch: {e}")))
}

/// Convert `Vec<GwasInfo>` into a single-output [`PortOutputs`].
async fn gwasinfo_to_output(
    ctx: &NodeCtx,
    rows: Vec<GwasInfo>,
    endpoint: &str,
) -> Result<PortOutputs, DagError> {
    tracing::info!("OpenGWAS {}: {} records returned", endpoint, rows.len());
    let batch = build_gwasinfo_batch(&rows)?;
    let session = ctx.session();
    let df = session
        .read_batch(batch)
        .map_err(|e| DagError::Schedule(format!("failed to read gwasinfo batch: {e}")))?;
    let mut res: PortOutputs = HashMap::new();
    res.insert(0, df);
    Ok(res)
}

// ===========================================================================
// 1. Associations  —  POST /associations
// ===========================================================================

const KIND_ASSOC: &str = "source_opengwas_associations";

/// Spec for [`OpengwasAssociationsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasAssociationsSpec {
    /// Variants as rsID or chr:pos (hg19/b37), e.g. `["rs1205", "7:105561135"]`.
    pub variant: Vec<String>,
    /// GWAS study IDs, e.g. `["ieu-a-2", "ukb-b-19953"]`.
    pub id: Vec<String>,
    /// Look for proxies: `1` (yes) or `0` (no). Default `0`.
    #[serde(default)]
    pub proxies: Option<i32>,
    /// Reference population for proxies. Default `"EUR"`.
    #[serde(default)]
    pub population: Option<String>,
    /// Minimum LD r² for a proxy. Default `0.8`.
    #[serde(default)]
    pub r2: Option<f64>,
}

/// Source node: variant × study associations from OpenGWAS `/associations`.
#[derive(Clone)]
pub struct OpengwasAssociationsNode {
    meta: NodePorts,
    spec: OpengwasAssociationsSpec,
}

pub struct OpengwasAssociationsNodeFactory;

impl NodeFactory for OpengwasAssociationsNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_ASSOC
    }
    fn desc(&self) -> &'static str {
        "Fetches variant–study associations from OpenGWAS /associations as a table."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/associations` endpoint for \
        specific variant–study associations (beta, se, p-value, alleles) and \
        emits them as a DataFrame.\n\n\
        Output schema is inferred from the API response — typically \
        `rsid, chr, position, ea, nea, eaf, beta, se, pval`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasAssociationsSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasAssociationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasAssociationsNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<OpengwasAssociationsSpec>(spec, KIND_ASSOC)?;
        let out = ctx.output_var.to_string();
        let variants = r_vec(&s.variant);
        let ids = r_vec(&s.id);
        let code = vec![
            format!("# OpenGWAS associations: variants × studies"),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("{out} <- ieugwasr::associations("),
            format!("  variants = c({variants}),"),
            format!("  id = c({ids})"),
            format!(")"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasAssociationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_ASSOC
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        let resp = client
            .associations(&AssociationsRequest {
                variant: self.spec.variant.clone(),
                id: self.spec.id.clone(),
                proxies: self.spec.proxies,
                population: self.spec.population.clone(),
                r2: self.spec.r2,
                align_alleles: None,
                palindromes: None,
                maf_threshold: None,
                commercial_approval_received: None,
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /associations failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/associations").await
    }
}

// ===========================================================================
// 2. PheWAS  —  POST /phewas
// ===========================================================================

const KIND_PHEWAS: &str = "source_opengwas_phewas";

fn default_phewas_pval() -> f64 {
    0.01
}

/// Spec for [`OpengwasPhewasNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasPhewasSpec {
    /// Variant identifiers (rsID, chr:pos, or chr:pos range on hg19/b37).
    pub variant: Vec<String>,
    /// P-value threshold (must ≤ 0.01). Default `0.01`.
    #[serde(default = "default_phewas_pval")]
    pub pval: f64,
    /// Restrict to specific study indexes. If empty, searches all.
    #[serde(default)]
    pub index_list: Option<Vec<String>>,
}

/// Source node: PheWAS results from OpenGWAS `/phewas`.
#[derive(Clone)]
pub struct OpengwasPhewasNode {
    meta: NodePorts,
    spec: OpengwasPhewasSpec,
}

pub struct OpengwasPhewasNodeFactory;

impl NodeFactory for OpengwasPhewasNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_PHEWAS
    }
    fn desc(&self) -> &'static str {
        "Performs PheWAS across all GWAS datasets via OpenGWAS /phewas."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/phewas` endpoint, scanning \
        the specified variants across all available GWAS datasets and returning \
        significant associations (p ≤ threshold).\n\n\
        Output schema is inferred — typically `id, rsid, chr, position, ea, nea, \
        beta, se, pval, n, trait`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasPhewasSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasPhewasSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasPhewasNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<OpengwasPhewasSpec>(spec, KIND_PHEWAS)?;
        let out = ctx.output_var.to_string();
        let variants = r_vec(&s.variant);
        let code = vec![
            format!("# OpenGWAS PheWAS (pval ≤ {})", s.pval),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("{out} <- ieugwasr::phewas("),
            format!("  variants = c({variants}),"),
            format!("  pval = {}", s.pval),
            format!(")"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasPhewasNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_PHEWAS
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        let resp = client
            .phewas(&PhewasRequest {
                variant: self.spec.variant.clone(),
                pval: Some(self.spec.pval),
                index_list: self.spec.index_list.clone(),
                commercial_approval_received: None,
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /phewas failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/phewas").await
    }
}

// ===========================================================================
// 3. GwasInfo by ID  —  POST /gwasinfo
// ===========================================================================

const KIND_GWASINFO: &str = "source_opengwas_gwasinfo";

/// Spec for [`OpengwasGwasinfoNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasGwasinfoSpec {
    /// GWAS study IDs to look up, e.g. `["ieu-a-2", "ukb-b-19953"]`.
    pub id: Vec<String>,
}

/// Source node: study metadata from OpenGWAS `/gwasinfo`.
#[derive(Clone)]
pub struct OpengwasGwasinfoNode {
    meta: NodePorts,
    spec: OpengwasGwasinfoSpec,
}

pub struct OpengwasGwasinfoNodeFactory;

impl NodeFactory for OpengwasGwasinfoNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_GWASINFO
    }
    fn desc(&self) -> &'static str {
        "Fetches GWAS study metadata by ID from OpenGWAS /gwasinfo."
    }
    fn doc(&self) -> &'static str {
        "A source node that fetches metadata for specific GWAS datasets by ID \
        and emits them as a DataFrame.\n\n\
        Output schema: `id, trait, group_name, category, subcategory, population, \
        sex, author, year, pmid, nsnp, sample_size, ncase, ncontrol, mr, priority, \
        is_nc, sd, unit, build, ontology, consortium, doi, …`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasGwasinfoSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasGwasinfoSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasGwasinfoNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<OpengwasGwasinfoSpec>(spec, KIND_GWASINFO)?;
        let out = ctx.output_var.to_string();
        let ids = r_vec(&s.id);
        let code = vec![
            format!("# OpenGWAS study metadata"),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("{out} <- ieugwasr::gwasinfo(id = c({ids}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasGwasinfoNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_GWASINFO
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        let rows = client
            .gwasinfo(&GwasInfoRequest {
                id: self.spec.id.clone(),
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /gwasinfo failed: {e}")))?;
        gwasinfo_to_output(node_ctx, rows, "/gwasinfo").await
    }
}

// ===========================================================================
// 4. GwasInfo search  —  SQL LIKE search on cached metadata
// ===========================================================================

const KIND_GWASINFO_SEARCH: &str = "source_opengwas_gwasinfo_search";

fn default_search_field() -> String {
    "trait".to_string()
}
fn default_search_limit() -> i64 {
    50
}

/// Spec for [`OpengwasGwasinfoSearchNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasGwasinfoSearchSpec {
    /// Search keyword (case-insensitive substring match).
    pub keyword: String,
    /// Column to search: `"trait"` (default), `"author"`, or `"population"`.
    #[serde(default = "default_search_field")]
    pub field: String,
    /// Maximum results to return. Default `50`.
    #[serde(default = "default_search_limit")]
    pub limit: i64,
    /// Column to sort by: `nsnp`, `sample_size`, `year`, `ncase`, etc.
    #[serde(default)]
    pub sort_by: Option<String>,
    /// Sort order: `"desc"` (default) or `"asc"`.
    #[serde(default)]
    pub sort_order: Option<String>,
}

/// Source node: keyword search across cached GWAS datasets.
#[derive(Clone)]
pub struct OpengwasGwasinfoSearchNode {
    meta: NodePorts,
    spec: OpengwasGwasinfoSearchSpec,
}

pub struct OpengwasGwasinfoSearchNodeFactory;

impl NodeFactory for OpengwasGwasinfoSearchNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_GWASINFO_SEARCH
    }
    fn desc(&self) -> &'static str {
        "Searches cached GWAS datasets by keyword via OpenGWAS."
    }
    fn doc(&self) -> &'static str {
        "A source node that searches the OpenGWAS cached metadata by keyword \
        (SQL LIKE on an indexed column) and emits matching datasets as a \
        DataFrame.\n\n\
        Output schema is the same as `source_opengwas_gwasinfo`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasGwasinfoSearchSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasGwasinfoSearchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasGwasinfoSearchNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<OpengwasGwasinfoSearchSpec>(spec, KIND_GWASINFO_SEARCH)?;
        let out = ctx.output_var.to_string();
        let code = vec![
            format!(
                "# OpenGWAS dataset search: \"{}\" in {}",
                s.keyword, s.field
            ),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("# Fetch all metadata then filter client-side"),
            format!("_all <- ieugwasr::gwasinfo()"),
            format!(
                "{out} <- _all[grepl(\"{}\", _all${}, ignore.case = TRUE), ]",
                s.keyword, s.field
            ),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasGwasinfoSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_GWASINFO_SEARCH
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        // Map external "trait" → internal column "trait_".
        let db_field = if self.spec.field == "trait" {
            "trait_"
        } else {
            &self.spec.field
        };
        let sort_by = self.spec.sort_by.as_deref().map(|s| {
            if s == "trait" {
                "trait_"
            } else {
                s
            }
        });
        let rows = client
            .gwasinfo_search(
                &self.spec.keyword,
                db_field,
                self.spec.limit,
                sort_by,
                self.spec.sort_order.as_deref(),
            )
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS gwasinfo_search failed: {e}")))?;
        gwasinfo_to_output(node_ctx, rows, "gwasinfo_search").await
    }
}

// ===========================================================================
// 5. Variants by rsID  —  POST /variants/rsid
// ===========================================================================

const KIND_VARIANTS_RSID: &str = "source_opengwas_variants_rsid";

/// Spec for [`OpengwasVariantsRsidNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasVariantsRsidSpec {
    /// Variant rs IDs, e.g. `["rs1205", "rs234"]`.
    pub rsid: Vec<String>,
}

/// Source node: variant annotations by rsID from OpenGWAS `/variants/rsid`.
#[derive(Clone)]
pub struct OpengwasVariantsRsidNode {
    meta: NodePorts,
    spec: OpengwasVariantsRsidSpec,
}

pub struct OpengwasVariantsRsidNodeFactory;

impl NodeFactory for OpengwasVariantsRsidNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_VARIANTS_RSID
    }
    fn desc(&self) -> &'static str {
        "Fetches variant annotations by rsID from OpenGWAS /variants/rsid."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/variants/rsid` endpoint for \
        variant annotations (chromosome, position, alleles) by rsID.\n\n\
        Output schema is inferred — typically `name, chr, position, ref, alt`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasVariantsRsidSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasVariantsRsidSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasVariantsRsidNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<OpengwasVariantsRsidSpec>(spec, KIND_VARIANTS_RSID)?;
        let out = ctx.output_var.to_string();
        let rsids = r_vec(&s.rsid);
        let code = vec![
            format!("# OpenGWAS variant annotations by rsID"),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("{out} <- ieugwasr::variants_rsid(c({rsids}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasVariantsRsidNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_VARIANTS_RSID
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        let resp = client
            .variants_rsid(&VariantsRsidRequest {
                rsid: self.spec.rsid.clone(),
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /variants/rsid failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/variants/rsid").await
    }
}

// ===========================================================================
// 6. Variants by chr:pos  —  POST /variants/chrpos
// ===========================================================================

const KIND_VARIANTS_CHRPOS: &str = "source_opengwas_variants_chrpos";

/// Spec for [`OpengwasVariantsChrposNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasVariantsChrposSpec {
    /// chr:pos strings on hg19/b37, e.g. `["7:105561135", "10:44865737"]`.
    pub chrpos: Vec<String>,
    /// Search radius in bp around each locus. Default `0`.
    #[serde(default)]
    pub radius: Option<i32>,
}

/// Source node: variant annotations by chr:pos from OpenGWAS `/variants/chrpos`.
#[derive(Clone)]
pub struct OpengwasVariantsChrposNode {
    meta: NodePorts,
    spec: OpengwasVariantsChrposSpec,
}

pub struct OpengwasVariantsChrposNodeFactory;

impl NodeFactory for OpengwasVariantsChrposNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_VARIANTS_CHRPOS
    }
    fn desc(&self) -> &'static str {
        "Fetches variant annotations by chr:pos from OpenGWAS /variants/chrpos."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/variants/chrpos` endpoint for \
        variant annotations by chromosome:position (hg19/b37), optionally with a \
        search radius.\n\n\
        Output schema is inferred — typically `name, chr, position, ref, alt`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasVariantsChrposSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasVariantsChrposSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasVariantsChrposNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<OpengwasVariantsChrposSpec>(spec, KIND_VARIANTS_CHRPOS)?;
        let out = ctx.output_var.to_string();
        let chrpos = r_vec(&s.chrpos);
        let code = vec![
            format!("# OpenGWAS variant annotations by chr:pos"),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("{out} <- ieugwasr::variants_chrpos(c({chrpos}))"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasVariantsChrposNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_VARIANTS_CHRPOS
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        let resp = client
            .variants_chrpos(&VariantsChrposRequest {
                chrpos: self.spec.chrpos.clone(),
                radius: self.spec.radius,
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /variants/chrpos failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/variants/chrpos").await
    }
}

// ===========================================================================
// 7. LD clump  —  POST /ld/clump
// ===========================================================================

const KIND_LD_CLUMP: &str = "source_opengwas_ld_clump";

fn default_clump_r2() -> f64 {
    0.001
}
fn default_clump_kb() -> i32 {
    5000
}
fn default_clump_pop() -> String {
    "EUR".to_string()
}

/// Spec for [`OpengwasLdClumpNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasLdClumpSpec {
    /// rs IDs to clump.
    pub rsid: Vec<String>,
    /// P-values for each SNP (same length as `rsid`).
    pub pval: Vec<f64>,
    /// Significance threshold. Default `5e-8`.
    #[serde(default)]
    pub pthresh: Option<f64>,
    /// LD r² threshold for clumping. Default `0.001`.
    #[serde(default = "default_clump_r2")]
    pub r2: f64,
    /// Clumping window size in kb. Default `5000`.
    #[serde(default = "default_clump_kb")]
    pub kb: i32,
    /// Reference population: `EUR`, `SAS`, `EAS`, `AFR`, `AMR`. Default `EUR`.
    #[serde(default = "default_clump_pop")]
    pub pop: String,
}

/// Source node: LD-clumped independent loci from OpenGWAS `/ld/clump`.
#[derive(Clone)]
pub struct OpengwasLdClumpNode {
    meta: NodePorts,
    spec: OpengwasLdClumpSpec,
}

pub struct OpengwasLdClumpNodeFactory;

impl NodeFactory for OpengwasLdClumpNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_LD_CLUMP
    }
    fn desc(&self) -> &'static str {
        "Performs LD clumping via OpenGWAS /ld/clump and returns independent loci."
    }
    fn doc(&self) -> &'static str {
        "A source node that calls the OpenGWAS `/ld/clump` endpoint to clump a \
        set of SNPs (with associated p-values) into independent loci using 1000 \
        Genomes reference data.\n\n\
        Output schema is inferred — typically `rsid, chr, position, pval, n, clump, \
        total, prop, kp, kp_2`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasLdClumpSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasLdClumpSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasLdClumpNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<OpengwasLdClumpSpec>(spec, KIND_LD_CLUMP)?;
        let out = ctx.output_var.to_string();
        let rsids = r_vec(&s.rsid);
        let pvals = r_vec_f64(&s.pval);
        let code = vec![
            format!("# OpenGWAS LD clumping (r2={}, kb={}, pop=\"{}\")", s.r2, s.kb, s.pop),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("_dat <- data.frame(rsid = c({rsids}), pval = c({pvals}))"),
            format!("{out} <- ieugwasr::ld_clump("),
            format!("  dat = _dat,"),
            format!("  clump_kb = {}, clump_r2 = {}, plink_bin = NULL", s.kb, s.r2),
            format!(")"),
        ];
        Ok(dag_core::codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasLdClumpNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_LD_CLUMP
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        let resp = client
            .ld_clump(&LdClumpRequest {
                rsid: self.spec.rsid.clone(),
                pval: self.spec.pval.clone(),
                pthresh: self.spec.pthresh,
                r2: Some(self.spec.r2),
                kb: Some(self.spec.kb),
                pop: Some(self.spec.pop.clone()),
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /ld/clump failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/ld/clump").await
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- extract_rows --

    #[test]
    fn extract_rows_flat_array() {
        let val = serde_json::json!([
            {"rsid": "rs1", "pval": 1e-8},
            {"rsid": "rs2", "pval": 1e-9},
        ]);
        let rows = extract_rows(&val);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn extract_rows_keyed_object() {
        let val = serde_json::json!({
            "ieu-a-2": [{"rsid": "rs1"}],
            "ukb-b-1": [{"rsid": "rs2"}, {"rsid": "rs3"}],
        });
        let rows = extract_rows(&val);
        assert_eq!(rows.len(), 3);
    }

    // -- infer_columns --

    #[test]
    fn infer_columns_priority_order() {
        let rows = extract_rows(&serde_json::json!([
            {"samplesize": 1000, "rsid": "rs1", "pval": 5e-8, "beta": 0.1, "zzz": "extra"},
        ]));
        let cols = infer_columns(&rows);
        assert_eq!(cols[0], "rsid");
        assert_eq!(cols.last().unwrap(), "zzz");
    }

    // -- build_json_batch --

    #[test]
    fn build_batch_infers_types() {
        let rows = extract_rows(&serde_json::json!([
            {"rsid": "rs1", "chr": 1, "position": 12345, "beta": 0.05, "pval": 5e-8},
            {"rsid": "rs2", "chr": 2, "position": 67890, "beta": -0.03, "pval": 1e-9},
        ]));
        let batch = build_json_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 2);

        let chr = batch
            .column_by_name("chr")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(chr.value(0), 1);
        assert_eq!(chr.value(1), 2);

        let beta = batch
            .column_by_name("beta")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((beta.value(0) - 0.05).abs() < 1e-10);

        let rsid = batch
            .column_by_name("rsid")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(rsid.value(0), "rs1");
    }

    #[test]
    fn build_batch_handles_nulls() {
        let rows = extract_rows(&serde_json::json!([
            {"rsid": "rs1", "pval": 5e-8},
            {"rsid": "rs2", "pval": null},
        ]));
        let batch = build_json_batch(&rows).unwrap();
        let pval = batch
            .column_by_name("pval")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((pval.value(0) - 5e-8).abs() < 1e-20);
        assert!(pval.is_null(1));
    }

    #[test]
    fn build_batch_empty_rows_errors() {
        let rows: Vec<Value> = vec![];
        assert!(build_json_batch(&rows).is_err());
    }

    // -- build_gwasinfo_batch --

    #[test]
    fn gwasinfo_batch_basic() {
        let rows = vec![
            GwasInfo {
                id: Some("ieu-a-2".into()),
                trait_: Some("CAD".into()),
                year: Some(2015),
                nsnp: Some(9455_552),
                sample_size: Some(184_305),
                sd: Some(1.0),
                ..Default::default()
            },
            GwasInfo {
                id: Some("ukb-b-19953".into()),
                trait_: Some("BMI".into()),
                year: Some(2020),
                ..Default::default()
            },
        ];
        let batch = build_gwasinfo_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 2);

        let id = batch
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(id.value(0), "ieu-a-2");
        assert_eq!(id.value(1), "ukb-b-19953");

        let year = batch
            .column_by_name("year")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(year.value(0), 2015);
        assert_eq!(year.value(1), 2020);

        let sd = batch
            .column_by_name("sd")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((sd.value(0) - 1.0).abs() < f64::EPSILON);
        assert!(sd.is_null(1));
    }

    #[test]
    fn gwasinfo_batch_empty_errors() {
        let rows: Vec<GwasInfo> = vec![];
        assert!(build_gwasinfo_batch(&rows).is_err());
    }

    // -- spec deserialization --

    #[test]
    fn associations_spec_deserialises() {
        let json = serde_json::json!({
            "variant": ["rs1205"],
            "id": ["ieu-a-2"]
        });
        let spec: OpengwasAssociationsSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.variant, vec!["rs1205"]);
        assert_eq!(spec.id, vec!["ieu-a-2"]);
    }

    #[test]
    fn phewas_spec_defaults() {
        let json = serde_json::json!({"variant": ["rs1205"]});
        let spec: OpengwasPhewasSpec = serde_json::from_value(json).unwrap();
        assert!((spec.pval - 0.01).abs() < f64::EPSILON);
    }

    #[test]
    fn gwasinfo_search_spec_defaults() {
        let json = serde_json::json!({"keyword": "diabetes"});
        let spec: OpengwasGwasinfoSearchSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.field, "trait");
        assert_eq!(spec.limit, 50);
    }

    #[test]
    fn ld_clump_spec_defaults() {
        let json = serde_json::json!({"rsid": ["rs1"], "pval": [1e-8]});
        let spec: OpengwasLdClumpSpec = serde_json::from_value(json).unwrap();
        assert!((spec.r2 - 0.001).abs() < f64::EPSILON);
        assert_eq!(spec.kb, 5000);
        assert_eq!(spec.pop, "EUR");
    }
}
