//! KEGG REST source nodes for structured biological identifiers and mappings.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use kegg::KeggClient;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodeInput, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

const MAPPING_SOURCE: &str = "kegg_api";
const RELATION_BATCH_SIZE: usize = 100;

fn client(
    endpoint: Option<&str>,
    requests_per_second: Option<u32>,
) -> Result<KeggClient, DagError> {
    let client = KeggClient::with_rate_limit(requests_per_second.unwrap_or(3))
        .map_err(|error| DagError::Schedule(format!("source_kegg: invalid rate limit: {error}")))?;
    Ok(match endpoint {
        Some(url) => client.with_endpoint(url),
        None => client,
    })
}

fn output_port() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

fn optional_gene_input() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::DataFrame)
        .add_output_port(None)
}

fn gene_pathway_ports() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::DataFrame)
        .add_output_port(None)
        .add_output_port(None)
}

fn to_data_frame(ctx: &SessionContext, batch: RecordBatch) -> Result<DataFrame, DagError> {
    ctx.read_batch(batch)
        .map_err(|error| DagError::Schedule(format!("failed to read KEGG batch: {error}")))
}

fn strings(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    let values: Vec<Option<&str>> = rows.iter().map(Option::as_deref).collect();
    Arc::new(StringArray::from(values))
}

async fn fetch_relation_pairs(
    client: &KeggClient,
    operation: KeggRelationOperation,
    target: &str,
    source: &str,
) -> Result<Vec<(String, String)>, DagError> {
    let target = canonical_relation_target(operation, target);
    let pairs = match operation {
        KeggRelationOperation::Conv => client.conv(&target, source).await,
        KeggRelationOperation::Link => client.link(&target, source).await,
    }
    .map_err(|error| DagError::Schedule(format!("KEGG relation failed: {error}")))?;
    Ok(pairs
        .into_iter()
        .map(|pair| (pair.source, pair.target))
        .collect())
}

fn required_string_column(df: &DataFrame, name: &str, node: &str) -> Result<usize, DagError> {
    df.schema()
        .index_of_column_by_name(None, name)
        .ok_or_else(|| DagError::Schedule(format!("{node}: input table has no column {name:?}")))
}

async fn string_values(df: &DataFrame, name: &str, node: &str) -> Result<Vec<String>, DagError> {
    let index = required_string_column(df, name, node)?;
    let batches =
        df.clone().collect().await.map_err(|error| {
            DagError::Schedule(format!("failed to collect input table: {error}"))
        })?;
    let mut values = Vec::new();
    for batch in batches {
        let column = batch.column(index);
        let values_array = column
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "{node}: column {name:?} must be UTF-8 (found {})",
                    column.data_type()
                ))
            })?;
        for index in 0..values_array.len() {
            if !values_array.is_null(index) {
                values.push(values_array.value(index).to_string());
            }
        }
    }
    Ok(values)
}

fn build_pair_batch(
    rows: Vec<(String, String)>,
    first: &str,
    second: &str,
) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new(first, DataType::Utf8, false),
        Field::new(second, DataType::Utf8, false),
    ]));
    let (left, right): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .map(|(left, right)| (Some(left), Some(right)))
        .unzip();
    RecordBatch::try_new(schema, vec![strings(left), strings(right)])
        .map_err(|error| DagError::Schedule(format!("failed to build KEGG pair batch: {error}")))
}

fn build_entry_batch(rows: Vec<(String, String)>) -> Result<RecordBatch, DagError> {
    build_pair_batch(rows, "id", "description")
}

/// Spec for `source_kegg_search`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct KeggSearchSpec {
    /// KEGG database to search, e.g. `genes`, `hsa`, `compound`, or `pathway`.
    #[serde(default = "default_search_database")]
    pub database: String,
    /// Native KEGG query, for example `BAIAP2` or `"shiga toxin"`.
    pub query: String,
    /// Optional native `find` option such as `formula`, `exact_mass`, or `mol_weight`.
    #[serde(default)]
    pub option: Option<String>,
    /// Endpoint override for tests and private KEGG mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Process-local request limit. KEGG's academic limit is 3.
    #[serde(default = "default_rate_limit")]
    pub requests_per_second: u32,
}

fn default_search_database() -> String {
    "genes".to_string()
}

fn default_rate_limit() -> u32 {
    3
}

/// Search KEGG and emit identifier/description rows.
#[derive(Clone)]
pub struct KeggSearchNode {
    meta: NodePorts,
    spec: KeggSearchSpec,
}

pub struct KeggSearchNodeFactory;

impl NodeFactory for KeggSearchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_kegg_search"
    }

    fn desc(&self) -> &'static str {
        "Search KEGG entries and emit identifier/description rows."
    }

    fn doc(&self) -> &'static str {
        "A zero-input source node for KEGG `find`. Output schema: \
         `id, description`. The query uses native KEGG syntax; quoted phrases \
         and options such as `formula` are supported. Unlike `genes`, the \
         `pathway` database does not support exact quoted-phrase matching."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KeggSearchSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(KeggSearchNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for KeggSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kegg_search"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = client(
            self.spec.endpoint.as_deref(),
            Some(self.spec.requests_per_second),
        )?;
        let rows = client
            .find_with_option(
                &self.spec.database,
                &self.spec.query,
                self.spec.option.as_deref(),
            )
            .await
            .map_err(|error| DagError::Schedule(format!("KEGG find failed: {error}")))?;
        let batch = build_entry_batch(
            rows.into_iter()
                .map(|entry| (entry.id, entry.description))
                .collect(),
        )?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, to_data_frame(&ctx.session(), batch)?);
        Ok(outputs)
    }
}

/// Direction for `source_kegg_relations`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Deserialize)]
pub enum KeggRelationOperation {
    /// Resolve cross-database identifiers with KEGG `conv`.
    #[serde(rename = "conv")]
    Conv,
    /// Resolve database relationships with KEGG `link`.
    #[serde(rename = "link")]
    Link,
}

/// Spec for `source_kegg_relations`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct KeggRelationsSpec {
    /// API operation, either `conv` or `link`.
    pub operation: KeggRelationOperation,
    /// Target database, e.g. `pathway`, `ko`, or `ncbi-geneid`.
    ///
    /// `link` uses KEGG's full database names. The common abbreviation `rn`
    /// is accepted as an alias for `reaction`.
    pub target: String,
    /// Source database or selected entries, e.g. `hsa` or `hsa:10458`.
    pub source: String,
    /// Optional input column used as the source when connected.
    #[serde(default)]
    pub source_column: Option<String>,
    /// Endpoint override for tests and private KEGG mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Process-local request limit. KEGG's academic limit is 3.
    #[serde(default = "default_rate_limit")]
    pub requests_per_second: u32,
}

/// Fetch KEGG conversion or link relationships.
#[derive(Clone)]
pub struct KeggRelationsNode {
    meta: NodePorts,
    spec: KeggRelationsSpec,
}

pub struct KeggRelationsNodeFactory;

impl NodeFactory for KeggRelationsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_kegg_relations"
    }

    fn desc(&self) -> &'static str {
        "Fetch KEGG link or ID-conversion relationships as a table."
    }

    fn doc(&self) -> &'static str {
        "A structured source/compute node for KEGG `link` and `conv`. It emits \
         `source, target`. Connect an optional DataFrame input and set \
         `source_column` to constrain a database-wide request to selected IDs. \
         For `link`, use full target names such as `pathway`, `reaction`, \
         `ko`, `compound`, `glycan`, `enzyme`, or an organism code such as \
         `hsa` or `eco`; `rn` is accepted as an alias for `reaction`, while \
         returned reaction IDs still use the `rn:` prefix. For `conv`, common \
         targets include `ncbi-geneid`, `ncbi-proteinid`, and `uniprot`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KeggRelationsSpec)
    }

    fn ports(&self) -> NodePorts {
        optional_gene_input()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(KeggRelationsNode {
            meta: optional_gene_input(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for KeggRelationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kegg_relations"
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
        let client = client(
            self.spec.endpoint.as_deref(),
            Some(self.spec.requests_per_second),
        )?;
        let mut rows = Vec::new();
        match inputs.first() {
            Some(input) => {
                let column = self.spec.source_column.as_deref().ok_or_else(|| {
                    DagError::Schedule(
                        "source_kegg_relations: source_column is required with an input".into(),
                    )
                })?;
                let entries =
                    string_values(input.dataframe()?, column, "source_kegg_relations").await?;
                for source in entries.chunks(RELATION_BATCH_SIZE) {
                    if source.is_empty() {
                        continue;
                    }
                    let source = source.join("+");
                    rows.extend(
                        fetch_relation_pairs(
                            &client,
                            self.spec.operation,
                            &self.spec.target,
                            &source,
                        )
                        .await?,
                    );
                }
            }
            None => {
                rows.extend(
                    fetch_relation_pairs(
                        &client,
                        self.spec.operation,
                        &self.spec.target,
                        &self.spec.source,
                    )
                    .await?,
                );
            }
        }

        let batch = build_pair_batch(rows, "source", "target")?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, to_data_frame(&ctx.session(), batch)?);
        Ok(outputs)
    }
}

/// Pathway identifier style emitted in `set_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Deserialize)]
pub enum KeggPathwayIdStyle {
    /// Keep organism-specific IDs such as `hsa00010`.
    #[serde(rename = "organism")]
    Organism,
    /// Rewrite organism-specific IDs to reference maps such as `map00010`.
    #[serde(rename = "map")]
    Map,
}

/// Spec for `source_kegg_gene_pathways`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct KeggGenePathwaysSpec {
    /// Three- or four-letter KEGG organism code, e.g. `hsa` or `eco`.
    pub organism: String,
    /// Pathway ID convention, default `map`.
    #[serde(default = "default_pathway_id_style")]
    pub pathway_id_style: KeggPathwayIdStyle,
    /// Remove the `org:` prefix from gene identifiers.
    #[serde(default = "default_true")]
    pub strip_gene_prefix: bool,
    /// Include pathway names and classes by fetching the KEGG pathway hierarchy.
    #[serde(default = "default_true")]
    pub include_pathway_names: bool,
    /// Optional input column used to retain only selected gene IDs.
    #[serde(default)]
    pub gene_column: Option<String>,
    /// Endpoint override for tests and private KEGG mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Process-local request limit. KEGG's academic limit is 3.
    #[serde(default = "default_rate_limit")]
    pub requests_per_second: u32,
}

fn default_pathway_id_style() -> KeggPathwayIdStyle {
    KeggPathwayIdStyle::Map
}

fn default_true() -> bool {
    true
}

/// Emit a long-format gene-pathway annotation table for enrichment workflows.
#[derive(Clone)]
pub struct KeggGenePathwaysNode {
    meta: NodePorts,
    spec: KeggGenePathwaysSpec,
}

pub struct KeggGenePathwaysNodeFactory;

impl NodeFactory for KeggGenePathwaysNodeFactory {
    fn kind(&self) -> &'static str {
        "source_kegg_gene_pathways"
    }

    fn desc(&self) -> &'static str {
        "Fetch a KEGG organism gene/pathway mapping as a long annotation table."
    }

    fn doc(&self) -> &'static str {
        "A structured source node for pathway annotation. Output port 0 emits \
         `gene_id, gene_id_kegg, set_id, set_id_kegg, mapping_source` for the \
         generic `enrichment_ora` annotation input. Output port 1 emits \
         `set_id, set_id_kegg, set_name, set_class` for its metadata input. \
         `pathway_id_style=map` rewrites `hsa00010` to `map00010`; \
         `strip_gene_prefix=true` turns `hsa:10458` into `10458`. The default \
         `map` style matches `enrichment_ora`'s default excluded broad maps \
         `map01100`, `map01110`, and `map01120`. With \
         `include_pathway_names=true`, names and top-level classes come from \
         the KEGG `br08901` pathway hierarchy; with `false`, both metadata \
         fields are null."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KeggGenePathwaysSpec)
    }

    fn ports(&self) -> NodePorts {
        gene_pathway_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(KeggGenePathwaysNode {
            meta: gene_pathway_ports(),
            spec,
        }))
    }
}

#[derive(Debug)]
struct PathwayAnnotation {
    gene_id: String,
    gene_id_kegg: String,
    set_id: String,
    set_id_kegg: String,
}

#[derive(Debug)]
struct PathwayMetadata {
    set_id: String,
    set_id_kegg: String,
    set_name: Option<String>,
    set_class: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct BritePathway {
    name: String,
    class: String,
}

#[derive(Debug, Default, Deserialize)]
struct KeggBriteNode {
    #[serde(default)]
    name: String,
    #[serde(default)]
    children: Vec<KeggBriteNode>,
}

#[async_trait]
impl DagNode for KeggGenePathwaysNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kegg_gene_pathways"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let node = self.kind();
        let client = client(
            self.spec.endpoint.as_deref(),
            Some(self.spec.requests_per_second),
        )?;
        let organism = self.spec.organism.trim();
        if organism.is_empty() {
            return Err(DagError::Schedule(format!(
                "{node}: organism cannot be empty"
            )));
        }

        let pairs = client
            .link("pathway", organism)
            .await
            .map_err(|error| DagError::Schedule(format!("KEGG pathway mapping failed: {error}")))?;
        let selected = match inputs.first() {
            Some(input) => {
                let column = self.spec.gene_column.as_deref().ok_or_else(|| {
                    DagError::Schedule(format!(
                        "{node}: gene_column is required when an input is connected"
                    ))
                })?;
                Some(
                    string_values(input.dataframe()?, column, node)
                        .await?
                        .into_iter()
                        .collect::<BTreeSet<_>>(),
                )
            }
            None => None,
        };

        let mut annotations = Vec::with_capacity(pairs.len());
        let mut metadata = BTreeMap::new();
        let mut pathway_details = BTreeMap::new();
        if self.spec.include_pathway_names {
            let hierarchy = client
                .brite_json::<KeggBriteNode>("br:br08901")
                .await
                .map_err(|error| {
                    DagError::Schedule(format!("KEGG pathway metadata request failed: {error}"))
                })?;
            collect_pathway_hierarchy(&hierarchy, &mut pathway_details);
        }

        for pair in pairs {
            let gene_id_kegg = pair.source;
            let set_id_kegg = pair
                .target
                .strip_prefix("path:")
                .unwrap_or(&pair.target)
                .to_string();
            let gene_id = if self.spec.strip_gene_prefix {
                gene_id_kegg
                    .rsplit_once(':')
                    .map_or_else(|| gene_id_kegg.clone(), |(_, gene)| gene.to_string())
            } else {
                gene_id_kegg.clone()
            };
            if let Some(selected) = &selected
                && !selected.contains(&gene_id)
                && !selected.contains(&gene_id_kegg)
            {
                continue;
            }
            let set_id = normalize_pathway_id(&set_id_kegg, self.spec.pathway_id_style);
            let set_id_kegg_for_metadata = set_id_kegg.clone();
            let pathway_details = pathway_details.get(&pathway_reference_id(&set_id_kegg));
            annotations.push(PathwayAnnotation {
                gene_id,
                gene_id_kegg,
                set_id: set_id.clone(),
                set_id_kegg,
            });
            metadata
                .entry(set_id.clone())
                .or_insert_with(|| PathwayMetadata {
                    set_id,
                    set_id_kegg: set_id_kegg_for_metadata,
                    set_name: pathway_details.map(|details| details.name.clone()),
                    set_class: pathway_details.map(|details| details.class.clone()),
                });
        }

        reporter.info(format!(
            "KEGG mapping: {} gene/pathway rows for organism {organism}",
            annotations.len()
        ));
        let batch = build_annotation_batch(annotations)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, to_data_frame(&ctx.session(), batch)?);
        let metadata_batch = build_metadata_batch(metadata.into_values().collect())?;
        outputs.insert(1, to_data_frame(&ctx.session(), metadata_batch)?);
        Ok(outputs)
    }
}

fn collect_brite_pathways(
    node: &KeggBriteNode,
    top_class: Option<&str>,
    output: &mut BTreeMap<String, BritePathway>,
) {
    let top_class = if node.children.is_empty() {
        top_class
    } else {
        top_class.or(Some(node.name.as_str()))
    };
    if node.children.is_empty() {
        if let Some((id, name)) = parse_brite_pathway(&node.name) {
            output.entry(id).or_insert_with(|| BritePathway {
                name,
                class: top_class.unwrap_or_default().trim().to_string(),
            });
        }
        return;
    }

    for child in &node.children {
        collect_brite_pathways(child, top_class, output);
    }
}

fn collect_pathway_hierarchy(root: &KeggBriteNode, output: &mut BTreeMap<String, BritePathway>) {
    for category in &root.children {
        collect_brite_pathways(category, None, output);
    }
}

fn parse_brite_pathway(value: &str) -> Option<(String, String)> {
    let (id, name) = value.trim().split_once(char::is_whitespace)?;
    let digits = id.strip_prefix("map").unwrap_or(id);
    if digits.len() == 5 && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        Some((format!("map{digits}"), name.trim().to_string()))
    } else {
        None
    }
}

fn pathway_reference_id(value: &str) -> String {
    let value = value.trim().strip_prefix("path:").unwrap_or(value.trim());
    match value.find(|character: char| character.is_ascii_digit()) {
        Some(start) if start > 0 => format!("map{}", &value[start..]),
        _ => value.to_string(),
    }
}

fn normalize_pathway_id(value: &str, style: KeggPathwayIdStyle) -> String {
    match style {
        KeggPathwayIdStyle::Organism => value.to_string(),
        KeggPathwayIdStyle::Map => {
            if value.contains(char::is_numeric) && !value.starts_with("map") {
                let digits_start = value
                    .find(|character: char| character.is_ascii_digit())
                    .unwrap_or(0);
                format!("map{}", &value[digits_start..])
            } else {
                value.to_string()
            }
        }
    }
}

fn build_annotation_batch(rows: Vec<PathwayAnnotation>) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("gene_id", DataType::Utf8, false),
        Field::new("gene_id_kegg", DataType::Utf8, false),
        Field::new("set_id", DataType::Utf8, false),
        Field::new("set_id_kegg", DataType::Utf8, false),
        Field::new("mapping_source", DataType::Utf8, false),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            strings(rows.iter().map(|row| Some(row.gene_id.clone())).collect()),
            strings(
                rows.iter()
                    .map(|row| Some(row.gene_id_kegg.clone()))
                    .collect(),
            ),
            strings(rows.iter().map(|row| Some(row.set_id.clone())).collect()),
            strings(
                rows.iter()
                    .map(|row| Some(row.set_id_kegg.clone()))
                    .collect(),
            ),
            strings(
                rows.into_iter()
                    .map(|_| Some(MAPPING_SOURCE.to_string()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| DagError::Schedule(format!("failed to build KEGG annotation batch: {error}")))
}

fn build_metadata_batch(rows: Vec<PathwayMetadata>) -> Result<RecordBatch, DagError> {
    let names: Vec<_> = rows.iter().map(|row| row.set_name.clone()).collect();
    let classes: Vec<_> = rows.iter().map(|row| row.set_class.clone()).collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new("set_id", DataType::Utf8, false),
        Field::new("set_id_kegg", DataType::Utf8, false),
        Field::new("set_name", DataType::Utf8, true),
        Field::new("set_class", DataType::Utf8, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            strings(rows.iter().map(|row| Some(row.set_id.clone())).collect()),
            strings(
                rows.iter()
                    .map(|row| Some(row.set_id_kegg.clone()))
                    .collect(),
            ),
            strings(names),
            strings(classes),
        ],
    )
    .map_err(|error| {
        DagError::Schedule(format!(
            "failed to build KEGG pathway metadata batch: {error}"
        ))
    })
}

fn canonical_relation_target(operation: KeggRelationOperation, target: &str) -> String {
    let target = target.trim();
    if operation == KeggRelationOperation::Link && target.eq_ignore_ascii_case("rn") {
        "reaction".to_string()
    } else {
        target.to_string()
    }
}

// ===========================================================================
// Drug–drug interactions
// ===========================================================================

/// Spec for `source_kegg_ddi`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct KeggDdiSpec {
    /// KEGG drug entry or entry list to look up interactions for, e.g.
    /// `D00001` or `D00001+D00002`. Multiples are `+`-joined.
    pub entries: String,
    /// Endpoint override for tests and private KEGG mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Process-local request limit. KEGG's academic limit is 3.
    #[serde(default = "default_rate_limit")]
    pub requests_per_second: u32,
}

/// Source node emitting KEGG drug–drug interaction rows
/// (`ddi/{entries}`) as a table.
#[derive(Clone)]
pub struct KeggDdiNode {
    meta: NodePorts,
    spec: KeggDdiSpec,
}

pub struct KeggDdiNodeFactory;

impl NodeFactory for KeggDdiNodeFactory {
    fn kind(&self) -> &'static str {
        "source_kegg_ddi"
    }

    fn desc(&self) -> &'static str {
        "Fetches KEGG drug-drug interactions for one or more drug entries as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node over the KEGG `ddi` operation. `entries` is one KEGG          drug id or a `+`-joined list (e.g. `D00001+D00002`); the node emits          one row per recorded interaction pair. This is the pipeline form of          the `kegg_ddi` tool: drug, interacts_with, category, and the raw          description as typed columns.\n\n\
         Output schema: `drug, interacts_with, category, description`.\n\n\
         Rate limiting matches the tooling (3 requests/second academic \
         limit); override with `requests_per_second`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KeggDdiSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: KeggDdiSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KeggDdiNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for KeggDdiNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kegg_ddi"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let entries = self.spec.entries.trim().to_string();
        if entries.is_empty() {
            return Err(DagError::Schedule(
                "source_kegg_ddi requires `entries` (a KEGG drug id or a `+`-joined list)".into(),
            ));
        }
        let client = client(
            self.spec.endpoint.as_deref(),
            Some(self.spec.requests_per_second),
        )?;
        let rows: Vec<kegg::DrugInteraction> = client
            .ddi(&entries)
            .await
            .map_err(|error| DagError::Schedule(format!("KEGG ddi failed: {error}")))?;

        let batch = build_ddi_batch(rows)?;
        let dataframe = to_data_frame(&ctx.session(), batch)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

fn build_ddi_batch(rows: Vec<kegg::DrugInteraction>) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("drug", DataType::Utf8, true),
        Field::new("interacts_with", DataType::Utf8, true),
        Field::new("category", DataType::Utf8, true),
        Field::new("description", DataType::Utf8, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            strings(rows.iter().map(|row| Some(row.drug.clone())).collect()),
            strings(
                rows.iter()
                    .map(|row| Some(row.interacts_with.clone()))
                    .collect(),
            ),
            strings(rows.iter().map(|row| Some(row.category.clone())).collect()),
            strings(
                rows.iter()
                    .map(|row| Some(row.description.clone()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| DagError::Schedule(format!("failed to build KEGG ddi batch: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use dag_core::dag::node_event::NodeReporter;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_stub(responses: Vec<(&'static str, String)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let responses: VecDeque<_> = responses.into_iter().collect();
        tokio::spawn(async move {
            for (content_type, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    let read = socket.read(&mut buffer).await.unwrap();
                    request.extend_from_slice(&buffer[..read]);
                    if read == 0 || request.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        endpoint
    }

    async fn first_batch(outputs: &PortOutputs) -> RecordBatch {
        outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap()
            .remove(0)
    }

    fn utf8(batch: &RecordBatch, column: usize, row: usize) -> String {
        batch
            .column(column)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(row)
            .to_string()
    }

    #[tokio::test]
    async fn search_node_emits_kegg_rows() {
        let endpoint = spawn_stub(vec![(
            "text/plain",
            "hsa:10458\tBAIAP2 adapter protein\n".to_string(),
        )])
        .await;
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let spec = serde_json::json!({
            "database": "genes",
            "query": "BAIAP2",
            "endpoint": endpoint,
            "requests_per_second": 100
        });
        let mut node = KeggSearchNodeFactory.build(spec, ctx.clone()).unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let batch = first_batch(&outputs).await;
        assert_eq!(utf8(&batch, 0, 0), "hsa:10458");
        assert_eq!(utf8(&batch, 1, 0), "BAIAP2 adapter protein");
    }

    #[tokio::test]
    async fn pathway_node_emits_enrichment_annotation_shape() {
        let endpoint = spawn_stub(vec![
            (
                "text/plain",
                "hsa:10458\tpath:hsa04151\nhsa:10458\tpath:hsa04520\n".to_string(),
            ),
            (
                "application/json",
                r#"{"name":"br08901","children":[{"name":"Organismal Systems","children":[{"name":"Signal transduction","children":[{"name":"04151\tPI3K-Akt signaling pathway"}]}]},{"name":"Cellular Processes","children":[{"name":"Cellular community","children":[{"name":"04520\tAdherens junction"}]}]}]}"#.to_string(),
            ),
        ])
        .await;
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let spec = serde_json::json!({
            "organism": "hsa",
            "pathway_id_style": "map",
            "strip_gene_prefix": true,
            "include_pathway_names": true,
            "endpoint": endpoint,
            "requests_per_second": 100
        });
        let mut node = KeggGenePathwaysNodeFactory
            .build(spec, ctx.clone())
            .unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let annotation = first_batch(&outputs).await;
        assert_eq!(utf8(&annotation, 0, 0), "10458");
        assert_eq!(utf8(&annotation, 1, 0), "hsa:10458");
        assert_eq!(utf8(&annotation, 2, 0), "map04151");
        assert_eq!(utf8(&annotation, 2, 1), "map04520");
        assert_eq!(utf8(&annotation, 4, 0), "kegg_api");
        let metadata = outputs
            .dataframe(1)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap()
            .remove(0);
        assert_eq!(metadata.num_columns(), 4);
        assert_eq!(utf8(&metadata, 0, 0), "map04151");
        assert_eq!(utf8(&metadata, 1, 0), "hsa04151");
        assert_eq!(utf8(&metadata, 2, 0), "PI3K-Akt signaling pathway");
        assert_eq!(utf8(&metadata, 3, 0), "Organismal Systems");
    }

    #[tokio::test]
    async fn pathway_node_can_omit_pathway_metadata() {
        let endpoint = spawn_stub(vec![(
            "text/plain",
            "hsa:10458\tpath:hsa04151\n".to_string(),
        )])
        .await;
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let spec = serde_json::json!({
            "organism": "hsa",
            "include_pathway_names": false,
            "endpoint": endpoint,
            "requests_per_second": 100
        });
        let mut node = KeggGenePathwaysNodeFactory
            .build(spec, ctx.clone())
            .unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let metadata = outputs
            .dataframe(1)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap()
            .remove(0);
        assert_eq!(metadata.num_rows(), 1);
        assert!(metadata.column(2).is_null(0));
        assert!(metadata.column(3).is_null(0));
    }

    #[tokio::test]
    #[ignore = "requires KEGG network access"]
    async fn real_kegg_hsa_pathway_metadata_is_complete() {
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
        let spec = serde_json::json!({
            "organism": "hsa",
            "requests_per_second": 3
        });
        let mut node = KeggGenePathwaysNodeFactory
            .build(spec, ctx.clone())
            .unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let batches = outputs
            .dataframe(1)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        let row_count: usize = batches.iter().map(RecordBatch::num_rows).sum();
        assert_eq!(row_count, 372);

        let mut checked_example = false;
        for batch in &batches {
            let set_ids = batch
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let names = batch
                .column(2)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let classes = batch
                .column(3)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for row in 0..batch.num_rows() {
                assert!(!names.is_null(row));
                assert!(!classes.is_null(row));
                if set_ids.value(row) == "map00010" {
                    assert_eq!(names.value(row), "Glycolysis / Gluconeogenesis");
                    assert_eq!(classes.value(row), "Metabolism");
                    checked_example = true;
                }
            }
        }
        assert!(checked_example);
    }

    #[test]
    fn normalizes_organism_pathways_to_reference_maps() {
        assert_eq!(
            normalize_pathway_id("hsa00010", KeggPathwayIdStyle::Map),
            "map00010"
        );
        assert_eq!(
            normalize_pathway_id("hsa00010", KeggPathwayIdStyle::Organism),
            "hsa00010"
        );
        assert_eq!(
            normalize_pathway_id("map00010", KeggPathwayIdStyle::Map),
            "map00010"
        );
    }

    #[test]
    fn maps_reaction_abbreviation_only_for_link() {
        assert_eq!(
            canonical_relation_target(KeggRelationOperation::Link, " rn "),
            "reaction"
        );
        assert_eq!(
            canonical_relation_target(KeggRelationOperation::Conv, "rn"),
            "rn"
        );
    }

    #[test]
    fn parses_pathway_hierarchy_names_and_top_level_classes() {
        let hierarchy = serde_json::from_str::<KeggBriteNode>(
            r#"{"name":"br08901","children":[{"name":"Metabolism","children":[{"name":"Carbohydrate metabolism","children":[{"name":"00010\tGlycolysis / Gluconeogenesis"}]}]}]}"#,
        )
        .unwrap();
        let mut details = BTreeMap::new();
        collect_pathway_hierarchy(&hierarchy, &mut details);

        assert_eq!(
            details.get("map00010"),
            Some(&BritePathway {
                name: "Glycolysis / Gluconeogenesis".to_string(),
                class: "Metabolism".to_string()
            })
        );
    }
}
