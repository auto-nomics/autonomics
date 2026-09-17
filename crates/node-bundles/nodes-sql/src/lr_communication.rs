//! Ligand-receptor communication scoring over a cluster mean table.

use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::table_transforms::{float_values, utf8_values};
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, DataBundleBinding, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

pub const LR_COMMUNICATION_SCORE_KIND: &str = "lr_communication_score";
pub const DEFAULT_LR_TABLE_BUNDLE: &str = "lrdb.cellphonedb.v5";
pub const DEFAULT_LR_TABLE_FILE: &str = "lr_pairs.parquet";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LrDirection {
    All,
    A2b,
    B2a,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComplexAggregation {
    Mean,
    Min,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LrCommunicationScoreSpec {
    /// Use the published CellPhoneDB LR catalog bundle when input port 0 is not wired.
    #[serde(default = "default_true")]
    pub use_catalog_lr_table: bool,
    #[serde(default = "default_lr_table_bundle")]
    pub lr_table_bundle: String,
    #[serde(default = "default_lr_table_file")]
    pub lr_table_file: String,
    /// Explicit LR table path override. Takes precedence over the catalog bundle.
    #[serde(default)]
    pub lr_table_path: Option<String>,
    /// Group rows sharing an interaction id/partner pair into one complex-aware interaction.
    #[serde(default = "default_true")]
    pub group_complexes: bool,
    /// Require every gene represented in a complex to be present in the mean table.
    #[serde(default = "default_true")]
    pub require_complete_complex: bool,
    #[serde(default = "default_complex_aggregation")]
    pub complex_aggregation: ComplexAggregation,
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

fn default_true() -> bool {
    true
}
fn default_lr_table_bundle() -> String {
    DEFAULT_LR_TABLE_BUNDLE.into()
}
fn default_lr_table_file() -> String {
    DEFAULT_LR_TABLE_FILE.into()
}
fn default_complex_aggregation() -> ComplexAggregation {
    ComplexAggregation::Mean
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
    fallback_lr_table_path: Option<String>,
}

fn node_error(message: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: LR_COMMUNICATION_SCORE_KIND.into(),
        msg: message.into(),
    }
}

fn aggregate_values(values: &[f64], aggregation: ComplexAggregation) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    match aggregation {
        ComplexAggregation::Mean => Some(values.iter().sum::<f64>() / values.len() as f64),
        ComplexAggregation::Min => values.iter().copied().reduce(f64::min),
    }
}

fn validate_bundle_relative_path(path: &str, field: &str) -> Result<(), String> {
    if path.trim().is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(format!("{field} must be a safe relative bundle path"));
    }
    Ok(())
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
    interaction_id: Option<String>,
    ligand_partner: Option<String>,
    receptor_partner: Option<String>,
    extra: Vec<Option<String>>,
}

#[derive(Debug, Clone)]
struct LrInteraction {
    id: String,
    ligand: String,
    receptor: String,
    ligand_genes: Vec<String>,
    receptor_genes: Vec<String>,
    extra: Vec<Option<String>>,
}

#[derive(Debug, Clone)]
struct ScoreRecord {
    interaction_index: usize,
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
            fallback_lr_table_path: None,
        }
    }

    pub fn new_with_fallback(
        spec: LrCommunicationScoreSpec,
        fallback_lr_table_path: Option<String>,
    ) -> Self {
        Self {
            ports: port_layout(),
            spec,
            fallback_lr_table_path,
        }
    }

    async fn default_lr_batches(&self, ctx: &NodeCtx) -> Result<Vec<RecordBatch>, DagError> {
        let path = self.fallback_lr_table_path.as_deref().ok_or_else(|| {
            node_error(
                "input port 0 is not wired and no default ligand-receptor table is configured",
            )
        })?;
        let session = ctx.session();
        session
            .register_parquet(
                "__lr_default",
                path,
                datafusion::prelude::ParquetReadOptions::default(),
            )
            .await
            .map_err(|error| {
                node_error(format!(
                    "cannot register default LR table `{path}`: {error}"
                ))
            })?;
        session
            .table("__lr_default")
            .await
            .map_err(|error| {
                node_error(format!("cannot resolve default LR table `{path}`: {error}"))
            })?
            .collect()
            .await
            .map_err(|error| node_error(format!("cannot read default LR table `{path}`: {error}")))
    }

    async fn read_lr_records(&self, batches: &[RecordBatch]) -> Result<Vec<LrRecord>, DagError> {
        let fields = batches
            .first()
            .map(|batch| batch.schema().fields().clone())
            .ok_or_else(|| node_error("ligand-receptor table has no batches"))?;
        let ligand = column_index(&fields, &self.spec.ligand_column, true)?.unwrap();
        let receptor = column_index(&fields, &self.spec.receptor_column, true)?.unwrap();
        let interaction_id = fields.find("interaction_id").map(|(index, _)| index);
        let ligand_partner = fields.find("ligand_partner").map(|(index, _)| index);
        let receptor_partner = fields.find("receptor_partner").map(|(index, _)| index);
        let mut extra = Vec::with_capacity(self.spec.extra_columns.len());
        for name in &self.spec.extra_columns {
            extra.push(column_index(&fields, name, true)?.unwrap());
        }
        let mut records = Vec::new();
        for batch in batches {
            let ligands = utf8_values(batch.column(ligand))?;
            let receptors = utf8_values(batch.column(receptor))?;
            let interaction_ids = interaction_id
                .map(|index| utf8_values(batch.column(index)))
                .transpose()?;
            let ligand_partners = ligand_partner
                .map(|index| utf8_values(batch.column(index)))
                .transpose()?;
            let receptor_partners = receptor_partner
                .map(|index| utf8_values(batch.column(index)))
                .transpose()?;
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
                    interaction_id: interaction_ids
                        .as_ref()
                        .and_then(|values| values[row].clone()),
                    ligand_partner: ligand_partners
                        .as_ref()
                        .and_then(|values| values[row].clone()),
                    receptor_partner: receptor_partners
                        .as_ref()
                        .and_then(|values| values[row].clone()),
                    extra: extras.iter().map(|values| values[row].clone()).collect(),
                });
            }
        }
        Ok(records)
    }

    fn group_lr_records(&self, records: &[LrRecord]) -> Vec<LrInteraction> {
        if !self.spec.group_complexes {
            return records
                .iter()
                .map(|record| LrInteraction {
                    id: format!("{}:{}", record.ligand, record.receptor),
                    ligand: record.ligand.clone(),
                    receptor: record.receptor.clone(),
                    ligand_genes: vec![record.ligand.clone()],
                    receptor_genes: vec![record.receptor.clone()],
                    extra: record.extra.clone(),
                })
                .collect();
        }

        let mut grouped: BTreeMap<String, Vec<&LrRecord>> = BTreeMap::new();
        for record in records {
            let key = record
                .interaction_id
                .clone()
                .unwrap_or_else(|| format!("{}:{}", record.ligand, record.receptor));
            grouped.entry(key).or_default().push(record);
        }
        grouped
            .into_iter()
            .map(|(id, records)| {
                let ligand_genes = records
                    .iter()
                    .map(|record| record.ligand.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let receptor_genes = records
                    .iter()
                    .map(|record| record.receptor.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let first = records[0];
                let ligand = if ligand_genes.len() == 1 {
                    ligand_genes[0].clone()
                } else {
                    first
                        .ligand_partner
                        .clone()
                        .unwrap_or_else(|| ligand_genes.join("+"))
                };
                let receptor = if receptor_genes.len() == 1 {
                    receptor_genes[0].clone()
                } else {
                    first
                        .receptor_partner
                        .clone()
                        .unwrap_or_else(|| receptor_genes.join("+"))
                };
                LrInteraction {
                    id,
                    ligand,
                    receptor,
                    ligand_genes,
                    receptor_genes,
                    extra: first.extra.clone(),
                }
            })
            .collect()
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
        Ok((clusters, means, fractions))
    }

    fn aggregate_genes(
        &self,
        genes: &[String],
        cluster: &str,
        values: &HashMap<(String, String), f64>,
    ) -> Option<f64> {
        let mut selected = Vec::with_capacity(genes.len());
        for gene in genes {
            match values.get(&(gene.clone(), cluster.to_string())) {
                Some(value) => selected.push(*value),
                None if self.spec.require_complete_complex => return None,
                None => {}
            }
        }
        aggregate_values(&selected, self.spec.complex_aggregation)
    }

    fn build_records(
        &self,
        interactions: &[LrInteraction],
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
        for (interaction_index, interaction) in interactions.iter().enumerate() {
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
                    let (Some(ligand_mean), Some(receptor_mean)) = (
                        self.aggregate_genes(&interaction.ligand_genes, ligand_cluster, means),
                        self.aggregate_genes(&interaction.receptor_genes, receptor_cluster, means),
                    ) else {
                        continue;
                    };
                    let score = ligand_mean * receptor_mean;
                    if self.spec.min_score.is_some_and(|minimum| score < minimum) {
                        continue;
                    }
                    let pct_product = match (
                        self.aggregate_genes(&interaction.ligand_genes, ligand_cluster, fractions),
                        self.aggregate_genes(
                            &interaction.receptor_genes,
                            receptor_cluster,
                            fractions,
                        ),
                    ) {
                        (Some(left), Some(right)) => Some((left / 100.0) * (right / 100.0)),
                        _ => None,
                    };
                    records.push(ScoreRecord {
                        interaction_index,
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
        .add_optional_input_port_of_type(PortType::DataFrame)
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
        if inputs.is_empty()
            || inputs.iter().any(|input| input.port > 1)
            || !inputs.iter().any(|input| input.port == 1)
        {
            return Err(node_error(
                "lr_communication_score requires cluster_mean_table on port 1 and accepts an optional lr_table on port 0",
            ));
        }
        let lr_batches = if let Some(input) = inputs.iter().find(|input| input.port == 0) {
            input.dataframe()?.clone().collect().await?
        } else {
            self.default_lr_batches(ctx).await?
        };
        let mean_batches = inputs
            .iter()
            .find(|input| input.port == 1)
            .expect("validated port 1 input")
            .dataframe()?
            .clone()
            .collect()
            .await?;
        let lr_records = self.read_lr_records(&lr_batches).await?;
        let interactions = self.group_lr_records(&lr_records);
        let (clusters, means, fractions) = self.read_cluster_means(&mean_batches).await?;
        let mut records = self.build_records(&interactions, &clusters, &means, &fractions)?;

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
                    let interaction = &interactions[record.interaction_index];
                    let ligand_values = interaction
                        .ligand_genes
                        .iter()
                        .map(|gene| permuted[&(gene.clone(), record.source.clone())])
                        .collect::<Vec<_>>();
                    let receptor_values = interaction
                        .receptor_genes
                        .iter()
                        .map(|gene| permuted[&(gene.clone(), record.target.clone())])
                        .collect::<Vec<_>>();
                    let (Some(ligand), Some(receptor)) = (
                        aggregate_values(&ligand_values, self.spec.complex_aggregation),
                        aggregate_values(&receptor_values, self.spec.complex_aggregation),
                    ) else {
                        continue;
                    };
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
            Field::new("interaction_id", DataType::Utf8, true),
            Field::new("ligand_genes", DataType::Utf8, true),
            Field::new("receptor_genes", DataType::Utf8, true),
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
                    .map(|record| interactions[record.interaction_index].ligand.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| interactions[record.interaction_index].receptor.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| interactions[record.interaction_index].id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| {
                        interactions[record.interaction_index]
                            .ligand_genes
                            .join(";")
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                records
                    .iter()
                    .map(|record| {
                        interactions[record.interaction_index]
                            .receptor_genes
                            .join(";")
                    })
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
                    .map(|record| interactions[record.interaction_index].extra[index].clone())
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
        "Input 1 is a long cluster mean table (`cluster`, `gene`, `mean_expression`, \
        optionally `pct_expressed`). Input 0 is an optional ligand-receptor table; \
        when omitted it defaults to the published CellPhoneDB v5 `lr_pairs.parquet` \
        catalog resource and can be overridden with `lr_table_path` or `lr_table_bundle`. \
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

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        vec![DataBundleBinding::new("lr_table", DEFAULT_LR_TABLE_BUNDLE)]
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
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
        if parsed.use_catalog_lr_table {
            if parsed.lr_table_bundle.trim().is_empty() {
                return Err("lr_table_bundle cannot be empty".into());
            }
            validate_bundle_relative_path(&parsed.lr_table_file, "lr_table_file")
                .map_err(dag_core::registry::error::Error::Unknown)?;
        }
        if parsed
            .lr_table_path
            .as_deref()
            .is_some_and(|path| path.trim().is_empty())
        {
            return Err("lr_table_path cannot be empty when provided".into());
        }
        let fallback = if let Some(path) = parsed.lr_table_path.clone() {
            Some(path)
        } else if parsed.use_catalog_lr_table {
            let bundle = node_ctx.bound_data_bundle("lr_table")?;
            Some(format!(
                "vfs://{}/{}",
                bundle.vpath.trim_end_matches('/'),
                parsed.lr_table_file.trim_start_matches('/')
            ))
        } else {
            None
        };
        Ok(Box::new(LrCommunicationScoreNode::new_with_fallback(
            parsed, fallback,
        )))
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let parsed: LrCommunicationScoreSpec = serde_json::from_value(spec)?;
        if !parsed.use_catalog_lr_table || parsed.lr_table_path.is_some() {
            return Ok(Vec::new());
        }
        Ok(vec![DataBundleBinding::new(
            "lr_table",
            parsed.lr_table_bundle,
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Array;
    use dag_core::node::{DataBundle, DataBundleCatalog};
    use datafusion::prelude::SessionContext;

    fn frame(batch: RecordBatch) -> datafusion::prelude::DataFrame {
        SessionContext::new().read_batch(batch).unwrap()
    }

    #[test]
    fn defaults_to_catalog_lr_table_and_optional_input() {
        let factory = LrCommunicationScoreNodeFactory;
        let bindings = factory
            .data_bundles_for_spec(serde_json::json!({}))
            .unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].binding, "lr_table");
        assert_eq!(bindings[0].bundle_id, DEFAULT_LR_TABLE_BUNDLE);
        let ports = factory.ports();
        assert!(!ports.input_port(0).unwrap().required);
        assert!(ports.input_port(1).unwrap().required);

        let catalog = Arc::new(
            DataBundleCatalog::from_bundles([DataBundle::new(
                DEFAULT_LR_TABLE_BUNDLE,
                "test LR table",
                "/bundles/lrdb.cellphonedb.v5",
            )])
            .unwrap(),
        );
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None)
            .with_data_bundle_catalog(catalog);
        let mut registry = dag_core::registry::NodeRegistry::new(ctx);
        registry.register(Box::new(factory));
        let node = registry
            .build_node(LR_COMMUNICATION_SCORE_KIND, serde_json::json!({}))
            .unwrap();
        let node = node
            .as_any()
            .downcast_ref::<LrCommunicationScoreNode>()
            .unwrap();
        assert_eq!(
            node.fallback_lr_table_path.as_deref(),
            Some("vfs:///bundles/lrdb.cellphonedb.v5/lr_pairs.parquet")
        );
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
            use_catalog_lr_table: false,
            lr_table_bundle: DEFAULT_LR_TABLE_BUNDLE.into(),
            lr_table_file: DEFAULT_LR_TABLE_FILE.into(),
            lr_table_path: None,
            group_complexes: true,
            require_complete_complex: true,
            complex_aggregation: ComplexAggregation::Mean,
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

    #[tokio::test]
    async fn reads_fallback_lr_table_when_port_zero_is_omitted() {
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
        let path = std::env::temp_dir().join(format!(
            "lr-communication-default-{}-{}.parquet",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        lr.write_parquet(
            path.to_string_lossy().as_ref(),
            datafusion::dataframe::DataFrameWriteOptions::new().with_single_file_output(true),
            None::<datafusion::config::TableParquetOptions>,
        )
        .await
        .unwrap();

        let mean = frame(
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("cluster", DataType::Utf8, false),
                    Field::new("gene", DataType::Utf8, false),
                    Field::new("mean_expression", DataType::Float64, false),
                ])),
                vec![
                    Arc::new(StringArray::from(vec!["A", "A", "B", "B"])),
                    Arc::new(StringArray::from(vec!["L", "R", "L", "R"])),
                    Arc::new(Float64Array::from(vec![2.0, 3.0, 5.0, 7.0])),
                ],
            )
            .unwrap(),
        );
        let spec = LrCommunicationScoreSpec {
            use_catalog_lr_table: false,
            lr_table_bundle: DEFAULT_LR_TABLE_BUNDLE.into(),
            lr_table_file: DEFAULT_LR_TABLE_FILE.into(),
            lr_table_path: Some(path.to_string_lossy().into_owned()),
            group_complexes: true,
            require_complete_complex: true,
            complex_aggregation: ComplexAggregation::Mean,
            ligand_column: "ligand".into(),
            receptor_column: "receptor".into(),
            cluster_column: "cluster".into(),
            gene_column: "gene".into(),
            mean_column: "mean_expression".into(),
            pct_column: None,
            n_permutations: 0,
            random_state: 1,
            direction: LrDirection::All,
            source_clusters: Vec::new(),
            target_clusters: Vec::new(),
            min_score: None,
            extra_columns: Vec::new(),
        };
        let mut node = LrCommunicationScoreNode::new_with_fallback(
            spec,
            Some(path.to_string_lossy().into_owned()),
        );
        let outputs = node
            .execute(
                &NodeCtx::new(SessionContext::new().runtime_env(), None),
                &[NodeInput::new_dataframe(1, mean)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            2
        );
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn aggregates_complex_subunits_before_scoring() {
        let lr = frame(
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("interaction_id", DataType::Utf8, false),
                    Field::new("ligand", DataType::Utf8, false),
                    Field::new("receptor", DataType::Utf8, false),
                    Field::new("ligand_partner", DataType::Utf8, false),
                    Field::new("receptor_partner", DataType::Utf8, false),
                ])),
                vec![
                    Arc::new(StringArray::from(vec!["LR1", "LR1"])),
                    Arc::new(StringArray::from(vec!["L1", "L2"])),
                    Arc::new(StringArray::from(vec!["R", "R"])),
                    Arc::new(StringArray::from(vec!["L1_L2", "L1_L2"])),
                    Arc::new(StringArray::from(vec!["R", "R"])),
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
                ])),
                vec![
                    Arc::new(StringArray::from(vec!["A", "A", "A", "B", "B", "B"])),
                    Arc::new(StringArray::from(vec!["L1", "L2", "R", "L1", "L2", "R"])),
                    Arc::new(Float64Array::from(vec![2.0, 4.0, 3.0, 5.0, 7.0, 10.0])),
                ],
            )
            .unwrap(),
        );
        let spec = LrCommunicationScoreSpec {
            use_catalog_lr_table: false,
            lr_table_bundle: DEFAULT_LR_TABLE_BUNDLE.into(),
            lr_table_file: DEFAULT_LR_TABLE_FILE.into(),
            lr_table_path: None,
            group_complexes: true,
            require_complete_complex: true,
            complex_aggregation: ComplexAggregation::Mean,
            ligand_column: "ligand".into(),
            receptor_column: "receptor".into(),
            cluster_column: "cluster".into(),
            gene_column: "gene".into(),
            mean_column: "mean_expression".into(),
            pct_column: None,
            n_permutations: 0,
            random_state: 0,
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
        let batches = outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        let scores = batches[0]
            .column_by_name("score")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        let mut values = scores.values().to_vec();
        values.sort_by(f64::total_cmp);
        assert_eq!(values, vec![18.0, 30.0]);
    }
}
