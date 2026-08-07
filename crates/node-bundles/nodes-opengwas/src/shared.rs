//! Shared JSON → Arrow helpers used by all OpenGWAS source nodes.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use datafusion::common::HashMap;
use serde_json::Value;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::NodePorts;
use dag_core::registry::NodeCtx;

use opengwas::types::GwasInfo;

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

/// Build an Arrow batch from a list of [`GwasInfo`] records.
///
/// Every field of `GwasInfo` becomes a column; string fields → `Utf8`,
/// integer fields → `Int64`, `sd` → `Float64`.
pub(crate) fn build_gwasinfo_batch(rows: &[GwasInfo]) -> Result<RecordBatch, DagError> {
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

/// Create an OpenGWAS client from the environment.
pub(crate) fn make_client() -> Result<opengwas::OpengwasClient, DagError> {
    opengwas::OpengwasClient::new(None).map_err(|e| {
        DagError::Schedule(format!(
            "failed to create OpenGWAS client (is OPENGWAS_TOKEN set?): {e}"
        ))
    })
}

/// A [`NodePorts`] with a single output port.
pub(crate) fn single_output_port() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

/// Convert a JSON response into a single-output [`PortOutputs`] via dynamic
/// schema inference. Shared by all JSON-returning source nodes.
pub(crate) async fn json_to_output(
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

/// Convert `Vec<GwasInfo>` into a single-output [`PortOutputs`].
pub(crate) async fn gwasinfo_to_output(
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
