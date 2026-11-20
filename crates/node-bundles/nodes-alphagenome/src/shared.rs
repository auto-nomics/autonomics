//! Shared helpers for the AlphaGenome node bundle.

use std::sync::Arc;

use alphagenome::{AlphaGenomeClient, VariantScores};
use arrow_array::{
    Array, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::NodePorts;
use dag_core::registry::NodeCtx;

/// Create an AlphaGenome client from the environment.
pub(crate) async fn make_client() -> Result<AlphaGenomeClient, DagError> {
    AlphaGenomeClient::from_env().await.map_err(|e| {
        DagError::Schedule(format!(
            "failed to create AlphaGenome client (is ALPHAGENOME_API_KEY set?): {e}"
        ))
    })
}

/// A [`NodePorts`] with a single output port.
pub(crate) fn single_output_port() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

/// Collect the string values of one column across input batches, dropping
/// nulls/blanks and deduplicating (first-seen order).
pub(crate) fn string_column_values(
    node_type: &str,
    batches: &[RecordBatch],
    column: &str,
) -> Result<Vec<String>, DagError> {
    let err = |msg: String| DagError::NodeError {
        node_type: node_type.into(),
        msg,
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for batch in batches {
        let arr = batch.column_by_name(column).ok_or_else(|| {
            err(format!(
                "input has no column '{column}' (available: {})",
                batch
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let values: Vec<Option<&str>> = if let Some(a) = arr.as_any().downcast_ref::<StringArray>()
        {
            (0..a.len())
                .map(|i| if a.is_null(i) { None } else { Some(a.value(i)) })
                .collect()
        } else if let Some(a) = arr.as_any().downcast_ref::<arrow_array::LargeStringArray>() {
            (0..a.len())
                .map(|i| if a.is_null(i) { None } else { Some(a.value(i)) })
                .collect()
        } else if let Some(a) = arr.as_any().downcast_ref::<arrow_array::StringViewArray>() {
            (0..a.len())
                .map(|i| if a.is_null(i) { None } else { Some(a.value(i)) })
                .collect()
        } else {
            return Err(err(format!(
                "column '{column}' must be a string column (Utf8/LargeUtf8/Utf8View), found {}",
                arr.data_type()
            )));
        };
        for v in values.into_iter().flatten() {
            let v = v.trim();
            if !v.is_empty() && seen.insert(v.to_string()) {
                out.push(v.to_string());
            }
        }
    }
    Ok(out)
}

/// Build the AIP-160 Atlas filter from scorer names and optional ontology
/// CURIEs, mirroring the Python SDK's `atlas_utils.build_filter`. Ontology
/// filters are relaxed for scorers without ontology metadata.
pub(crate) fn build_filter(scorers: &[String], ontology_terms: &[String]) -> String {
    let mut clauses: Vec<String> = Vec::new();

    if !scorers.is_empty() {
        let parts: Vec<String> = scorers
            .iter()
            .map(|s| format!("scores.variant_scorer.name = \"{s}\""))
            .collect();
        clauses.push(format!("({})", parts.join(" OR ")));
    }

    if !ontology_terms.is_empty() {
        let mut parts: Vec<String> = Vec::new();
        for curie in ontology_terms {
            let Some((ontology_name, id)) = parse_curie(curie) else {
                continue;
            };
            parts.push(format!(
                "(scores.metadata.tracks.metadata.ontology_term.ontology_type = \
                 {ontology_name} AND \
                 scores.metadata.tracks.metadata.ontology_term.id = {id})"
            ));
        }
        if !parts.is_empty() {
            // Ignore the ontology filter for scorers without ontology metadata.
            clauses.push(format!(
                "(-scores.metadata.tracks.metadata.ontology_term:* OR ({}))",
                parts.join(" OR ")
            ));
        }
    }

    clauses.join(" AND ")
}

/// `UBERON:0001157` → (`ONTOLOGY_TYPE_UBERON`, 1157).
fn parse_curie(curie: &str) -> Option<(&'static str, i64)> {
    let (prefix, id) = curie.trim().split_once(':')?;
    let id: i64 = id.trim_start_matches('0').parse().ok()?;
    let name = match prefix.trim().to_ascii_uppercase().as_str() {
        "CLO" => "ONTOLOGY_TYPE_CLO",
        "UBERON" => "ONTOLOGY_TYPE_UBERON",
        "CL" => "ONTOLOGY_TYPE_CL",
        "EFO" => "ONTOLOGY_TYPE_EFO",
        "NTR" => "ONTOLOGY_TYPE_NTR",
        _ => return None,
    };
    Some((name, id))
}

/// One tidy row: variant × scorer × value index (× track/gene metadata).
pub(crate) struct ScoreRow {
    pub chromosome: String,
    pub position: i64,
    pub reference: String,
    pub alternate: String,
    pub scorer: String,
    pub is_signed: bool,
    pub value_index: u32,
    pub raw_score: Option<f64>,
    pub quantile_score: Option<f64>,
    pub track: Option<String>,
    pub biosample: Option<String>,
    pub ontology_curie: Option<String>,
    pub gene_id: Option<String>,
    pub gene_name: Option<String>,
    /// Per-variant lookup failure (variant echo only, other fields null).
    pub error: Option<String>,
}

/// A row carrying only a per-variant error, so a miss or a malformed variant
/// string shows up in the output instead of failing the node.
pub(crate) fn error_row(
    chromosome: &str,
    position: i64,
    reference: &str,
    alternate: &str,
    message: String,
) -> ScoreRow {
    ScoreRow {
        chromosome: chromosome.to_owned(),
        position,
        reference: reference.to_owned(),
        alternate: alternate.to_owned(),
        scorer: String::new(),
        is_signed: false,
        value_index: 0,
        raw_score: None,
        quantile_score: None,
        track: None,
        biosample: None,
        ontology_curie: None,
        gene_id: None,
        gene_name: None,
        error: Some(message),
    }
}

/// Flatten a [`VariantScores`] into tidy rows.
pub(crate) fn flatten_scores(scores: &VariantScores) -> Vec<ScoreRow> {
    let mut rows = Vec::new();
    for score in &scores.scores {
        let calibrated_len = score.calibrated_values.as_ref().map_or(0, |c| c.len());
        for index in 0..score.values.len().max(calibrated_len) {
            let raw = score.values.get(index).map(|v| *v as f64);
            let quantile = score
                .calibrated_values
                .as_ref()
                .and_then(|c| c.get(index))
                .map(|v| *v as f64);
            let track = score.tracks.get(index);
            let gene = score.genes.get(index);
            rows.push(ScoreRow {
                chromosome: scores.variant.chromosome.clone(),
                position: scores.variant.position,
                reference: scores.variant.reference_bases.clone(),
                alternate: scores.variant.alternate_bases.clone(),
                scorer: score.scorer.clone(),
                is_signed: score.is_signed,
                value_index: index as u32,
                raw_score: raw,
                quantile_score: quantile,
                track: track.map(|t| t.name.clone()),
                biosample: track.and_then(|t| t.biosample.clone()),
                ontology_curie: track.and_then(|t| t.ontology_curie.clone()),
                gene_id: gene.map(|g| g.gene_id.clone()),
                gene_name: gene.and_then(|g| g.name.clone()),
                error: None,
            });
        }
    }
    rows
}

/// Build the Arrow batch for tidy score rows with a fixed schema.
pub(crate) fn build_score_batch(rows: &[ScoreRow]) -> Result<RecordBatch, DagError> {
    if rows.is_empty() {
        return Err(DagError::Schedule(
            "AlphaGenome query returned no rows".into(),
        ));
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("chromosome", DataType::Utf8, false),
        Field::new("position", DataType::Int64, false),
        Field::new("ref", DataType::Utf8, false),
        Field::new("alt", DataType::Utf8, false),
        Field::new("scorer", DataType::Utf8, true),
        Field::new("is_signed", DataType::Boolean, true),
        Field::new("value_index", DataType::UInt32, true),
        Field::new("raw_score", DataType::Float64, true),
        Field::new("quantile_score", DataType::Float64, true),
        Field::new("track", DataType::Utf8, true),
        Field::new("biosample", DataType::Utf8, true),
        Field::new("ontology_curie", DataType::Utf8, true),
        Field::new("gene_id", DataType::Utf8, true),
        Field::new("gene_name", DataType::Utf8, true),
        Field::new("error", DataType::Utf8, true),
    ]));

    // `str_col!(field)` — Option<String> field; `str_col_req!(field)` —
    // required String field.
    macro_rules! str_col {
        ($field:ident) => {{
            let vals: Vec<Option<&str>> = rows.iter().map(|r| r.$field.as_deref()).collect();
            Arc::new(StringArray::from(vals)) as Arc<dyn Array>
        }};
    }
    macro_rules! str_col_req {
        ($field:ident) => {{
            let vals: Vec<&str> = rows.iter().map(|r| r.$field.as_str()).collect();
            Arc::new(StringArray::from(vals)) as Arc<dyn Array>
        }};
    }

    RecordBatch::try_new(
        schema,
        vec![
            str_col_req!(chromosome),
            Arc::new(Int64Array::from(
                rows.iter().map(|r| r.position).collect::<Vec<_>>(),
            )),
            str_col_req!(reference),
            str_col_req!(alternate),
            str_col_req!(scorer),
            Arc::new(BooleanArray::from(
                rows.iter().map(|r| r.is_signed).collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.value_index).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.raw_score).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.quantile_score).collect::<Vec<_>>(),
            )),
            str_col!(track),
            str_col!(biosample),
            str_col!(ontology_curie),
            str_col!(gene_id),
            str_col!(gene_name),
            str_col!(error),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build AlphaGenome batch: {e}")))
}

/// Like [`build_score_batch`] plus `tail_quantile` and `phred` columns
/// derived from the calibrated CDF.
pub(crate) fn build_avi_batch(
    rows: &[ScoreRow],
    tails: &[Option<f64>],
    phreds: &[Option<f64>],
) -> Result<RecordBatch, DagError> {
    let base = build_score_batch(rows)?;
    let mut fields: Vec<Arc<Field>> = base.schema().fields().to_vec();
    fields.push(Arc::new(Field::new(
        "tail_quantile",
        DataType::Float64,
        true,
    )));
    fields.push(Arc::new(Field::new("phred", DataType::Float64, true)));
    let mut arrays: Vec<Arc<dyn Array>> = base.columns().to_vec();
    arrays.push(Arc::new(Float64Array::from(tails.to_vec())));
    arrays.push(Arc::new(Float64Array::from(phreds.to_vec())));
    RecordBatch::try_new(Arc::new(arrow_schema::Schema::new(fields)), arrays)
        .map_err(|e| DagError::Schedule(format!("failed to build AVI batch: {e}")))
}

/// Emit tidy score rows as a single-output [`PortOutputs`].
pub(crate) async fn score_rows_to_output(
    ctx: &NodeCtx,
    rows: &[ScoreRow],
    endpoint: &str,
) -> Result<PortOutputs, DagError> {
    tracing::info!("AlphaGenome {}: {} rows returned", endpoint, rows.len());
    let batch = build_score_batch(rows)?;
    let session = ctx.session();
    let df = session
        .read_batch(batch)
        .map_err(|e| DagError::Schedule(format!("failed to read AlphaGenome batch: {e}")))?;
    let mut res: PortOutputs = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}
