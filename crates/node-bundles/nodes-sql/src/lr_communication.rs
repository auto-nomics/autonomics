//! Ligand-receptor communication scoring over a cluster mean table.

use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::table_transforms::{float_values, utf8_values};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

pub const LR_COMMUNICATION_SCORE_KIND: &str = "lr_communication_score";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LrDirection {
    All,
    A2b,
    B2a,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LrCommunicationScoreSpec {
    #[serde(default = "default_ligand_column")]
    pub ligand_column: String,
    #[serde(default = "default_receptor_column")]
    pub receptor_column: String,
    #[serde(default = "default_cluster_column")]
    pub cluster_column: String,
    #[serde(default = "default_gene_column")]
    pub gene_column: String,
    #[serde(default = "default_mean_column")]
    pub mean_column: String,
    #[serde(default = "default_pct_column")]
    pub pct_column: Option<String>,
    #[serde(default = "default_n_permutations")]
    pub n_permutations: u32,
    #[serde(default)]
    pub random_state: u64,
    #[serde(default = "default_direction")]
    pub direction: LrDirection,
    #[serde(default)]
    pub source_clusters: Vec<String>,
    #[serde(default)]
    pub target_clusters: Vec<String>,
    #[serde(default)]
    pub min_score: Option<f64>,
    #[serde(default)]
    pub extra_columns: Vec<String>,
}

fn default_ligand_column() -> String {
    "ligand".into()
}
fn default_receptor_column() -> String {
    "receptor".into()
}
fn default_cluster_column() -> String {
    "cluster".into()
}
fn default_gene_column() -> String {
    "gene".into()
}
fn default_mean_column() -> String {
    "mean_expression".into()
}
fn default_pct_column() -> Option<String> {
    Some("pct_expressed".into())
}
fn default_n_permutations() -> u32 {
    1000
}
fn default_direction() -> LrDirection {
    LrDirection::All
}

#[derive(Clone)]
pub struct LrCommunicationScoreNode {
    ports: NodePorts,
    spec: LrCommunicationScoreSpec,
}

fn node_error(message: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: LR_COMMUNICATION_SCORE_KIND.into(),
        msg: message.into(),
    }
}

fn column_index(
    fields: &arrow_schema::Fields,
    name: &str,
    required: bool,
) -> Result<Option<usize>, DagError> {
    let index = fields.find(name).map(|(index, _)| index);
    if index.is_none() && required {
        return Err(node_error(format!("required column `{name}` is absent")));
    }
    Ok(index)
}

#[derive(Debug, Clone)]
struct LrRecord {
    ligand: String,
    receptor: String,
    extra: Vec<Option<String>>,
}

#[derive(Debug, Clone)]
struct ScoreRecord {
    lr_index: usize,
    source: String,
    target: String,
    score: f64,
    pct_product: Option<f64>,
    pvalue: Option<f64>,
    pvalue_adj: Option<f64>,
}

#[derive(Clone)]
struct DeterministicRng(u64);

impl DeterministicRng {
    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.0 = state;
        state
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let swap = (self.next_u64() % (index as u64 + 1)) as usize;
            values.swap(index, swap);
        }
    }
}

fn bh_adjust(pvalues: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut indexed = pvalues
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.map(|p| (index, p)))
        .collect::<Vec<_>>();
    indexed.sort_by(|left, right| left.1.total_cmp(&right.1));
    let mut adjusted = vec![None; pvalues.len()];
    let valid = indexed.len();
    for rank_from_one in (1..=valid).rev() {
        let position = rank_from_one - 1;
        let (index, pvalue) = indexed[position];
        let value = (pvalue * valid as f64 / rank_from_one as f64).min(1.0);
        let value = if position + 1 < valid {
            value.min(adjusted[indexed[position + 1].0].unwrap_or(1.0))
        } else {
            value
        };
        adjusted[index] = Some(value);
    }
    adjusted
}

impl LrCommunicationScoreNode {
    pub fn new(spec: LrCommunicationScoreSpec) -> Self {
        Self {
            ports: port_layout(),
            spec,
        }
    }

    async fn read_lr_records(&self, batches: &[RecordBatch]) -> Result<Vec<LrRecord>, DagError> {
        let fields = batches
            .first()
            .map(|batch| batch.schema().fields().clone())
            .ok_or_else(|| node_error("ligand-receptor table has no batches"))?;
        let ligand = column_index(&fields, &self.spec.ligand_column, true)?.unwrap();
        let receptor = column_index(&fields, &self.spec.receptor_column, true)?.unwrap();
        let mut extra = Vec::with_capacity(self.spec.extra_columns.len());
        for name in &self.spec.extra_columns {
            extra.push(column_index(&fields, name, true)?.unwrap());
        }
        let mut records = Vec::new();
        for batch in batches {
            let ligands = utf8_values(batch.column(ligand))?;
            let receptors = utf8_values(batch.column(receptor))?;
            let extras = extra
                .iter()
                .map(|index| utf8_values(batch.column(*index)))
                .collect::<Result<Vec<_>, _>>()?;
            for row in 0..batch.num_rows() {
                let Some(ligand) = ligands[row].clone() else {
                    continue;
                };
                let Some(receptor) = receptors[row].clone() else {
                    continue;
                };
                records.push(LrRecord {
                    ligand,
                    receptor,
                    extra: extras.iter().map(|values| values[row].clone()).collect(),
                });
            }
        }
        Ok(records)
    }

    async fn read_cluster_means(
        &self,
        batches: &[RecordBatch],
    ) -> Result<
        (
            BTreeSet<String>,
            HashMap<(String, String), f64>,
            HashMap<(String, String), f64>,
        ),
        DagError,
    > {
        let fields = batches
            .first()
            .map(|batch| batch.schema().fields().clone())
            .ok_or_else(|| node_error("cluster mean table has no batches"))?;
        let cluster = column_index(&fields, &self.spec.cluster_column, true)?.unwrap();
        let gene = column_index(&fields, &self.spec.gene_column, true)?.unwrap();
        let mean = column_index(&fields, &self.spec.mean_column, true)?.unwrap();
        let pct = self
            .spec
            .pct_column
            .as_deref()
            .and_then(|name| fields.find(name).map(|(index, _)| index));
        let mut clusters = BTreeSet::new();
        let mut means = HashMap::new();
        let mut fractions = HashMap::new();
        for batch in batches {
            let cluster_values = utf8_values(batch.column(cluster))?;
            let gene_values = utf8_values(batch.column(gene))?;
            let mean_values = float_values(batch.column(mean))?;
            let pct_values = match pct {
                Some(index) => Some(float_values(batch.column(index))?),
                None => None,
            };
            for row in 0..batch.num_rows() {
                let (Some(cluster), Some(gene), Some(mean)) =
                    (&cluster_values[row], &gene_values[row], mean_values[row])
                else {
                    continue;
                };
                clusters.insert(cluster.clone());
                if means
                    .insert((gene.clone(), cluster.clone()), mean)
                    .is_some()
                {
                    return Err(node_error(format!(
                        "cluster mean table has duplicate gene/cluster row: `{gene}` / `{cluster}`"
                    )));
                }
                if let Some(values) = &pct_values
                    && let Some(value) = values[row]
                {
                    fractions.insert((gene.clone(), cluster.clone()), value);
                }
            }
        }
        let genes = means
            .keys()
            .map(|(gene, _)| gene.clone())
            .collect::<BTreeSet<_>>();
        for gene in &genes {
            for cluster in &clusters {
                if !means.contains_key(&(gene.clone(), cluster.clone())) {
                    return Err(node_error(format!(
                        "cluster mean table has no mean for gene `{gene}` in cluster `{cluster}`"
                    )));
                }
            }
        }
        Ok((clusters, means, fractions))
    }

    fn build_records(
        &self,
        lr_records: &[LrRecord],
        clusters: &BTreeSet<String>,
        means: &HashMap<(String, String), f64>,
        fractions: &HashMap<(String, String), f64>,
    ) -> Result<Vec<ScoreRecord>, DagError> {
        let all_clusters = clusters.iter().cloned().collect::<Vec<_>>();
        let source_set = if self.spec.source_clusters.is_empty() {
            None
        } else {
            Some(
                self.spec
                    .source_clusters
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>(),
            )
        };
        let target_set = if self.spec.target_clusters.is_empty() {
            None
        } else {
            Some(
                self.spec
                    .target_clusters
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>(),
            )
        };
        let mut records = Vec::new();
        for (lr_index, record) in lr_records.iter().enumerate() {
            for source in &all_clusters {
                for target in &all_clusters {
                    if source == target {
                        continue;
                    }
                    let (ligand_cluster, receptor_cluster) = match self.spec.direction {
                        LrDirection::All => (source, target),
                        LrDirection::A2b => (source, target),
                        LrDirection::B2a => (target, source),
                    };
                    if source_set.as_ref().is_some_and(|set| !set.contains(source))
                        || target_set.as_ref().is_some_and(|set| !set.contains(target))
                    {
                        continue;
                    }
                    let ligand_key = (record.ligand.clone(), ligand_cluster.clone());
                    let receptor_key = (record.receptor.clone(), receptor_cluster.clone());
                    let (Some(ligand_mean), Some(receptor_mean)) =
                        (means.get(&ligand_key), means.get(&receptor_key))
                    else {
                        continue;
                    };
                    let score = ligand_mean * receptor_mean;
                    if self.spec.min_score.is_some_and(|minimum| score < minimum) {
                        continue;
                    }
                    let pct_product =
                        match (fractions.get(&ligand_key), fractions.get(&receptor_key)) {
                            (Some(left), Some(right)) => Some((left / 100.0) * (right / 100.0)),
                            _ => None,
                        };
                    records.push(ScoreRecord {
                        lr_index,
                        source: ligand_cluster.clone(),
                        target: receptor_cluster.clone(),
                        score,
                        pct_product,
                        pvalue: None,
                        pvalue_adj: None,
                    });
                }
            }
        }
        if records.is_empty() {
            return Err(node_error(
                "no ligand-receptor pair had both genes in the cluster mean table",
            ));
        }
        Ok(records)
    }
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::DataFrame, "lr_table")
        .add_input_port_of_type_with_label(None, PortType::DataFrame, "cluster_mean_table")
        .add_output_port(None)
}

#[async_trait]
impl DagNode for LrCommunicationScoreNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        LR_COMMUNICATION_SCORE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if inputs.len() != 2 || inputs.iter().any(|input| input.port > 1) {
            return Err(node_error(
                "lr_communication_score requires inputs on ports 0 and 1 only",
            ));
        }
        let lr_batches = inputs[0].dataframe()?.clone().collect().await?;
        let mean_batches = inputs[1].dataframe()?.clone().collect().await?;
        let lr_records = self.read_lr_records(&lr_batches).await?;
        let (clusters, means, fractions) = self.read_cluster_means(&mean_batches).await?;
        let mut records = self.build_records(&lr_records, &clusters, &means, &fractions)?;

        if self.spec.n_permutations > 0 {
            let cluster_order = clusters.iter().cloned().collect::<Vec<_>>();
            let genes = means
                .keys()
                .map(|(gene, _)| gene.clone())
                .collect::<BTreeSet<_>>();
            let mut rng = DeterministicRng({
                let seed = self.spec.random_state;
                if seed == 0 { 0x9e3779b97f4a7c15 } else { seed }
            });
            let mut exceedances = vec![0_u32; records.len()];
            for _ in 0..self.spec.n_permutations {
                let mut permuted = HashMap::new();
                for gene in &genes {
                    let mut values = cluster_order
                        .iter()
                        .map(|cluster| means[&(gene.clone(), cluster.clone())])
                        .collect::<Vec<_>>();
                    rng.shuffle(&mut values);
                    for (cluster, value) in cluster_order.iter().zip(values) {
                        permuted.insert((gene.clone(), cluster.clone()), value);
                    }
                }
                for (index, record) in records.iter().enumerate() {
                    let lr = &lr_records[record.lr_index];
                    let ligand = permuted[&(lr.ligand.clone(), record.source.clone())];
                    let receptor = permuted[&(lr.receptor.clone(), record.target.clone())];
                    if ligand * receptor >= record.score {
                        exceedances[index] += 1;
                    }
                }
            }
            for (record, count) in records.iter_mut().zip(exceedances) {
                record.pvalue =
                    Some((count as f64 + 1.0) / (self.spec.n_permutations as f64 + 1.0));
            }
            let adjusted = bh_adjust(
                &records
                    .iter()
                    .map(|record| record.pvalue)
                    .collect::<Vec<_>>(),
            );
            for (record, pvalue) in records.iter_mut().zip(adjusted) {
                record.pvalue_adj = pvalue;
            }
        }

        let mut fields = vec![
            Field::new("ligand", DataType::Utf8, true),
            Field::new("receptor", DataType::Utf8, true),
            Field::new("ligand_cluster", DataType::Utf8, true),
            Field::new("receptor_cluster", DataType::Utf8, true),
            Field::new("score", DataType::Float64, true),
            Field::new("pct_expressed_product", DataType::Float64, true),
            Field::new("pvalue", DataType::Float64, true),
            Field::new("pvalue_adj", DataType::Float64, true),
        ];
        fields.extend(
            self.spec
                .extra_columns
                .iter()
                .map(|name| Field::new(name, DataType::Utf8, true)),
        );
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| lr_records[record.lr_index].ligand.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| lr_records[record.lr_index].receptor.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| record.source.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| record.target.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                records
                    .iter()
                    .map(|record| record.score)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                records
                    .iter()
                    .map(|record| record.pct_product)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                records
                    .iter()
                    .map(|record| record.pvalue)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                records
                    .iter()
                    .map(|record| record.pvalue_adj)
                    .collect::<Vec<_>>(),
            )),
        ];
        for index in 0..self.spec.extra_columns.len() {
            columns.push(Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| lr_records[record.lr_index].extra[index].clone())
                    .collect::<Vec<_>>(),
            )));
        }
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .map_err(|error| node_error(format!("cannot build score table: {error}")))?;
        let output = ctx.session().read_batch(batch)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, output);
        Ok(outputs)
    }
}

pub struct LrCommunicationScoreNodeFactory;

impl NodeFactory for LrCommunicationScoreNodeFactory {
    fn kind(&self) -> &'static str {
        LR_COMMUNICATION_SCORE_KIND
    }

    fn desc(&self) -> &'static str {
        "Scores ligand-receptor pairs using mean-expression products and label permutations."
    }

    fn doc(&self) -> &'static str {
        "Input 0 is a ligand-receptor table and input 1 is a long cluster mean \
        table (`cluster`, `gene`, `mean_expression`, optionally `pct_expressed`). \
        For every ordered pair of distinct clusters, score = ligand mean x receptor \
        mean. Permutations shuffle mean values across cluster labels within each \
        gene; p-values are exact one-sided add-one estimates and BH-adjusted."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LrCommunicationScoreSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let parsed: LrCommunicationScoreSpec = serde_json::from_value(spec)?;
        if parsed.n_permutations > 100_000 {
            return Err("n_permutations cannot exceed 100000".into());
        }
        if let Some(minimum) = parsed.min_score
            && !minimum.is_finite()
        {
            return Err("min_score must be finite".into());
        }
        if parsed.pct_column.as_deref() == Some(parsed.mean_column.as_str()) {
            return Err("pct_column and mean_column must differ".into());
        }
        Ok(Box::new(LrCommunicationScoreNode::new(parsed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn frame(batch: RecordBatch) -> datafusion::prelude::DataFrame {
        SessionContext::new().read_batch(batch).unwrap()
    }

    #[tokio::test]
    async fn scores_and_permutes_ordered_cluster_pairs() {
        let lr = frame(
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("ligand", DataType::Utf8, false),
                    Field::new("receptor", DataType::Utf8, false),
                ])),
                vec![
                    Arc::new(StringArray::from(vec!["L"])),
                    Arc::new(StringArray::from(vec!["R"])),
                ],
            )
            .unwrap(),
        );
        let mean = frame(
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("cluster", DataType::Utf8, false),
                    Field::new("gene", DataType::Utf8, false),
                    Field::new("mean_expression", DataType::Float64, false),
                    Field::new("pct_expressed", DataType::Float64, false),
                ])),
                vec![
                    Arc::new(StringArray::from(vec!["A", "A", "B", "B"])),
                    Arc::new(StringArray::from(vec!["L", "R", "L", "R"])),
                    Arc::new(Float64Array::from(vec![2.0, 3.0, 5.0, 7.0])),
                    Arc::new(Float64Array::from(vec![50.0, 40.0, 80.0, 60.0])),
                ],
            )
            .unwrap(),
        );
        let spec = LrCommunicationScoreSpec {
            ligand_column: "ligand".into(),
            receptor_column: "receptor".into(),
            cluster_column: "cluster".into(),
            gene_column: "gene".into(),
            mean_column: "mean_expression".into(),
            pct_column: Some("pct_expressed".into()),
            n_permutations: 99,
            random_state: 1,
            direction: LrDirection::All,
            source_clusters: Vec::new(),
            target_clusters: Vec::new(),
            min_score: None,
            extra_columns: Vec::new(),
        };
        let mut node = LrCommunicationScoreNode::new(spec);
        let outputs = node
            .execute(
                &NodeCtx::new(SessionContext::new().runtime_env(), None),
                &[
                    NodeInput::new_dataframe(0, lr),
                    NodeInput::new_dataframe(1, mean),
                ],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            2
        );
    }
}
