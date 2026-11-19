//! AlphaGenome Atlas variant scoring nodes.
//!
//! * `source_alphagenome_scores` — variants embedded in the spec
//! * `alphagenome_scores` — variants read from an upstream DataFrame column

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use alphagenome::VariantQuery;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::shared::{
    build_filter, flatten_scores, make_client, score_rows_to_output, single_output_port,
    ScoreRow,
};

fn default_scorers() -> Vec<String> {
    vec!["AVI_SCORE".to_owned()]
}

fn default_variant_column() -> String {
    "variant".into()
}

/// Common spec fields shared by both scoring nodes.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ScorersSpec {
    /// Atlas scorer names, e.g. `AVI_SCORE`, `RNA_SEQ`, `DNASE`,
    /// `AVI_SCORE_FEATURE_IMPORTANCE`. Default `["AVI_SCORE"]`. At most a
    /// handful per request keeps responses small.
    #[serde(default = "default_scorers")]
    pub scorers: Vec<String>,
    /// Optional ontology CURIEs (`UBERON:0001157` colon) restricting track
    /// scorers to tissues of interest; ignored for scorers without ontology
    /// metadata.
    #[serde(default)]
    pub ontology_terms: Vec<String>,
}

/// Spec for [`AlphagenomeScoresSourceNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct AlphagenomeScoresSourceSpec {
    /// GRCh38 SNVs in any accepted spelling (`chr22:36201698:A>C`,
    /// `22-36201698-A-C`, `chr22_36201698_A_C_b38`).
    pub variants: Vec<String>,
    #[serde(flatten)]
    pub scorers: ScorersSpec,
}

/// Source node: precomputed Atlas scores for a list of GRCh38 SNVs.
#[derive(Clone)]
pub struct AlphagenomeScoresSourceNode {
    meta: NodePorts,
    spec: AlphagenomeScoresSourceSpec,
}

pub struct AlphagenomeScoresSourceNodeFactory;

impl NodeFactory for AlphagenomeScoresSourceNodeFactory {
    fn kind(&self) -> &'static str {
        "source_alphagenome_scores"
    }
    fn desc(&self) -> &'static str {
        "Fetches precomputed AlphaGenome Atlas scores for GRCh38 SNVs \
        (default: the AVI score)."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the AlphaGenome Atlas for precomputed \
        variant scores. Variants are 1-based GRCh38 SNVs (`chr:pos:ref>alt`; \
        gnomAD and GTEx spellings are autodetected). By default returns the \
        AVI score; pass `scorers` for track-level scores (RNA_SEQ, DNASE, …) \
        and `ontology_terms` CURIEs to restrict to tissues.\n\n\
        Output (long format): `chromosome, position, ref, alt, scorer, \
        is_signed, value_index, raw_score, quantile_score, track, biosample, \
        ontology_curie, gene_id, gene_name, error`. Variants that fail \
        lookup produce a row with only the `error` column set.\n\n\
        Requires the `ALPHAGENOME_API_KEY` environment variable. Atlas scores \
        are SNV-only and GRCh38-only; use the model API for indels or mouse."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AlphagenomeScoresSourceSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: AlphagenomeScoresSourceSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AlphagenomeScoresSourceNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
}

/// Spec for [`AlphagenomeScoresNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct AlphagenomeScoresSpec {
    /// Name of the input DataFrame column holding variant strings
    /// (`chr:pos:ref>alt` and friends). Default `variant`.
    #[serde(default = "default_variant_column")]
    pub variant_column: String,
    #[serde(flatten)]
    pub scorers: ScorersSpec,
}

/// Scores node: like [`AlphagenomeScoresSourceNode`] but reading variants
/// from an upstream DataFrame column.
#[derive(Clone)]
pub struct AlphagenomeScoresNode {
    meta: NodePorts,
    spec: AlphagenomeScoresSpec,
}

pub struct AlphagenomeScoresNodeFactory;

impl NodeFactory for AlphagenomeScoresNodeFactory {
    fn kind(&self) -> &'static str {
        "alphagenome_scores"
    }
    fn desc(&self) -> &'static str {
        "Scores variants from an upstream DataFrame column with the \
        AlphaGenome Atlas (default: AVI)."
    }
    fn doc(&self) -> &'static str {
        "Takes a DataFrame with a variant column (`variant_column`, default \
        `variant`) holding 1-based GRCh38 SNV strings and queries the \
        AlphaGenome Atlas for each unique variant. Same scorers, output \
        schema and requirements as `source_alphagenome_scores` — see that \
        node's documentation."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AlphagenomeScoresSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port_of_type_with_label(None, PortType::DataFrame, "variants")
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: AlphagenomeScoresSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AlphagenomeScoresNode {
            meta: self.ports(),
            spec: s,
        }))
    }
}

/// Parse variant strings; failures keep the raw string alongside the message.
fn parse_variants(raw: &[String]) -> Vec<Result<VariantQuery, (String, String)>> {
    raw.iter()
        .map(|s| {
            s.parse::<VariantQuery>()
                .map_err(|e| (s.clone(), e.to_string()))
        })
        .collect()
}

async fn query_scores(
    node_ctx: &NodeCtx,
    variants: &[Result<VariantQuery, (String, String)>],
    scorers: &ScorersSpec,
    kind: &str,
) -> Result<PortOutputs, DagError> {
    let err = |msg: String| DagError::NodeError {
        node_type: kind.into(),
        msg,
    };
    let mut client = make_client().await?;
    let filter = build_filter(&scorers.scorers, &scorers.ontology_terms);

    let mut rows: Vec<ScoreRow> = Vec::new();
    for variant in variants {
        match variant {
            Ok(v) => {
                // One request per variant so a miss becomes a row-level error
                // instead of failing the whole node (mirrors the official
                // atlas_query script).
                match client.atlas_variant_scores(v, Some(&filter)).await {
                    Ok(scores) => rows.extend(flatten_scores(&scores)),
                    Err(e) => rows.push(crate::shared::error_row(
                        &v.chromosome,
                        v.position,
                        &v.reference_bases,
                        &v.alternate_bases,
                        e.to_string(),
                    )),
                }
            }
            Err((raw, msg)) => {
                rows.push(crate::shared::error_row(raw, 0, "", "", msg.clone()));
            }
        }
    }

    if rows.is_empty() {
        return Err(err("no variants to score".into()));
    }
    score_rows_to_output(node_ctx, &rows, "scores").await
}

#[async_trait]
impl DagNode for AlphagenomeScoresSourceNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "source_alphagenome_scores"
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
        if self.spec.variants.is_empty() {
            return Err(DagError::NodeError {
                node_type: self.kind().into(),
                msg: "spec has no variants".into(),
            });
        }
        let variants = parse_variants(&self.spec.variants);
        query_scores(node_ctx, &variants, &self.spec.scorers, self.kind()).await
    }
}

#[async_trait]
impl DagNode for AlphagenomeScoresNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "alphagenome_scores"
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
        let input = inputs.first().ok_or_else(|| {
            DagError::NodeError {
                node_type: self.kind().into(),
                msg: "no input: connect a DataFrame with variant strings to \
                      the 'variants' port"
                    .into(),
            }
        })?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: self.kind().into(),
                msg: format!("failed to collect input DataFrame: {e}"),
            })?;
        let variants = crate::shared::string_column_values(
            self.kind(),
            &batches,
            &self.spec.variant_column,
        )?;
        if variants.is_empty() {
            return Err(DagError::NodeError {
                node_type: self.kind().into(),
                msg: format!(
                    "column '{}' contains no usable variant strings",
                    self.spec.variant_column
                ),
            });
        }
        let parsed = parse_variants(&variants);
        query_scores(node_ctx, &parsed, &self.spec.scorers, self.kind()).await
    }
}
