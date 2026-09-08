//! `source_uniprot_idmap` — map IDs between databases via UniProt and emit
//! a DataFrame.

use std::sync::Arc;

use arrow_array::{Array, BooleanArray, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodeInput, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::UniProtClient;
use crate::types::{Entry, IdMappingResult};

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

/// Spec for [`UniprotIdmapNode`].
#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct UniprotIdmapSpec {
    /// Source database name, e.g. `"Ensembl"`, `"Gene_Name"`,
    /// `"RefSeq_Protein"`, `"UniProtKB_AC-ID"`.
    #[serde(default)]
    pub from_db: String,

    /// Target database name, e.g. `"UniProtKB"`, `"UniProtKB-Swiss-Prot"`.
    #[serde(default)]
    pub to_db: String,

    /// IDs to map when no input table is connected.
    #[serde(default)]
    pub ids: Option<Vec<String>>,

    /// Column of the connected input table holding the IDs to map. Required
    /// when an input is wired; must be a string column.
    #[serde(default)]
    pub id_column: Option<String>,
}

// ---------------------------------------------------------------------------
// Node + Factory
// ---------------------------------------------------------------------------

/// Source node mapping identifiers through the UniProt ID mapping service.
#[derive(Clone)]
pub struct UniprotIdmapNode {
    meta: NodePorts,
    spec: UniprotIdmapSpec,
}

pub struct UniprotIdmapNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::DataFrame)
        .add_output_port(None)
}

impl NodeFactory for UniprotIdmapNodeFactory {
    fn kind(&self) -> &'static str {
        "source_uniprot_idmap"
    }

    fn desc(&self) -> &'static str {
        "Map IDs between databases via UniProt and emit a table."
    }

    fn doc(&self) -> &'static str {
        "A source node that submits IDs to the UniProt ID mapping service \
        (server-side job, may take a few seconds) and emits the mappings as \
        a DataFrame. One optional DataFrame input port (a table whose \
        `id_column` holds the IDs to map); one output port.\n\n\
        When no input is connected, the IDs come from the spec's `ids` \
        list.\n\n\
        Common `from_db` values: UniProtKB_AC-ID, Gene_Name, Ensembl, \
        RefSeq_Protein, Entrez_Gene, PDB, HGNC. `to_db` is typically \
        `UniProtKB`.\n\n\
        Output schema: `id, mapped, accession, entry_name, protein_name, \
        gene_names, organism_name, taxon_id`. Unmapped IDs keep `mapped = \
        false` with null annotation columns."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(UniprotIdmapSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: UniprotIdmapSpec = serde_json::from_value(spec)?;
        Ok(Box::new(UniprotIdmapNode {
            meta: port_layout(),
            spec: node_spec,
        }))
    }
}

#[async_trait]
impl DagNode for UniprotIdmapNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_uniprot_idmap"
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
        let from = self.spec.from_db.trim();
        let to = self.spec.to_db.trim();
        if from.is_empty() || to.is_empty() {
            return Err(DagError::Schedule(
                "source_uniprot_idmap requires non-empty `from_db` and `to_db`".into(),
            ));
        }

        // IDs come from the wired input table when present, else the spec.
        let ids: Vec<String> = match inputs.first() {
            Some(input) => {
                let column = self.spec.id_column.as_deref().ok_or_else(|| {
                    DagError::Schedule(
                        "source_uniprot_idmap: `id_column` is required when an \
                         input table is connected"
                            .into(),
                    )
                })?;
                strings_from_column(input.dataframe()?, column).await?
            }
            None => self.spec.ids.clone().ok_or_else(|| {
                DagError::Schedule(
                    "source_uniprot_idmap: provide `ids` in the spec or connect \
                     an input table"
                        .into(),
                )
            })?,
        };
        if ids.is_empty() {
            return Err(DagError::Schedule(
                "source_uniprot_idmap: no IDs to map (empty spec list or empty \
                 input column)"
                    .into(),
            ));
        }

        let client = UniProtClient::new();
        let results = client
            .map_ids_json(from, to, &ids)
            .await
            .map_err(|e| DagError::Schedule(format!("UniProt ID mapping failed: {e}")))?;

        let session = ctx.session();
        let batch = build_idmap_batch(&results.results)?;
        let df = session
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read mapping batch: {e}")))?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ---------------------------------------------------------------------------
// Input column extraction
// ---------------------------------------------------------------------------

/// Collect the (non-null) string values of `column` from a DataFrame.
async fn strings_from_column(df: &DataFrame, column: &str) -> Result<Vec<String>, DagError> {
    let schema = df.schema();
    let idx = schema
        .index_of_column_by_name(None, column)
        .ok_or_else(|| {
            DagError::Schedule(format!(
                "source_uniprot_idmap: input table has no column named {column:?}"
            ))
        })?;

    let batches = df
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::Schedule(format!("failed to collect input table: {e}")))?;

    let mut values = Vec::new();
    for batch in &batches {
        let col = batch.column(idx);
        let arr = col.as_any().downcast_ref::<StringArray>().ok_or_else(|| {
            DagError::Schedule(format!(
                "source_uniprot_idmap: column {column:?} must be a string \
                     column (found {})",
                col.data_type()
            ))
        })?;
        for i in 0..arr.len() {
            if !arr.is_null(i) {
                values.push(arr.value(i).to_owned());
            }
        }
    }
    Ok(values)
}

// ---------------------------------------------------------------------------
// Arrow batch builder
// ---------------------------------------------------------------------------

fn build_idmap_batch(rows: &[IdMappingResult]) -> Result<RecordBatch, DagError> {
    let ids: Vec<Option<String>> = rows.iter().map(|r| Some(r.from.clone())).collect();
    let mapped: Vec<bool> = rows.iter().map(|r| r.to.is_some()).collect();
    let accessions: Vec<Option<String>> = rows
        .iter()
        .map(|r| r.to.as_ref().map(|e| e.primary_accession.clone()))
        .collect();
    let entry_names: Vec<Option<String>> = rows
        .iter()
        .map(|r| r.to.as_ref().map(|e| e.uni_protkb_id.clone()))
        .collect();
    let protein_names: Vec<Option<String>> = rows
        .iter()
        .map(|r| {
            r.to.as_ref()
                .and_then(|e| e.protein_name().map(str::to_owned))
        })
        .collect();
    let gene_names: Vec<Option<String>> = rows
        .iter()
        .map(|r| {
            r.to.as_ref()
                .map(|e| e.gene_names().join(", "))
                .filter(|s| !s.is_empty())
        })
        .collect();
    let organisms: Vec<Option<String>> = rows
        .iter()
        .map(|r| {
            r.to.as_ref()
                .map(|e| e.organism.scientific_name.clone().unwrap_or_default())
                .filter(|s| !s.is_empty())
        })
        .collect();
    let taxon_ids: Vec<Option<u64>> = rows
        .iter()
        .map(|r| r.to.as_ref().map(|e| e.organism.taxon_id))
        .collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, true),
        Field::new("mapped", DataType::Boolean, true),
        Field::new("accession", DataType::Utf8, true),
        Field::new("entry_name", DataType::Utf8, true),
        Field::new("protein_name", DataType::Utf8, true),
        Field::new("gene_names", DataType::Utf8, true),
        Field::new("organism_name", DataType::Utf8, true),
        Field::new("taxon_id", DataType::UInt64, true),
    ]));

    let str_array = |vals: Vec<Option<String>>| -> Arc<dyn Array> {
        let refs: Vec<Option<&str>> = vals.iter().map(|o| o.as_deref()).collect();
        Arc::new(StringArray::from(refs))
    };

    RecordBatch::try_new(
        schema,
        vec![
            str_array(ids),
            Arc::new(BooleanArray::from(mapped)),
            str_array(accessions),
            str_array(entry_names),
            str_array(protein_names),
            str_array(gene_names),
            str_array(organisms),
            Arc::new(UInt64Array::from(taxon_ids)),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build mapping batch: {e}")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;

    fn ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    fn entry_fixture() -> Entry {
        serde_json::from_str(
            r#"{
              "entryType": "UniProtKB reviewed (Swiss-Prot)",
              "primaryAccession": "P01308",
              "uniProtkbId": "INS_HUMAN",
              "organism": {"scientificName": "Homo sapiens", "taxonId": 9606},
              "proteinDescription": {"recommendedName": {"fullName": {"value": "Insulin"}}},
              "genes": [{"geneName": {"value": "INS"}}]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn build_batch_from_mapping_results() {
        let rows = vec![
            IdMappingResult {
                from: "ENST00000397029".into(),
                to: Some(entry_fixture()),
            },
            IdMappingResult {
                from: "NOPE".into(),
                to: None,
            },
        ];

        let batch = build_idmap_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 8);

        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(ids.value(0), "ENST00000397029");
        assert_eq!(ids.value(1), "NOPE");

        let mapped = batch
            .column(1)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        assert!(mapped.value(0));
        assert!(!mapped.value(1));

        let accs = batch
            .column(2)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(accs.value(0), "P01308");
        assert!(accs.is_null(1));

        let taxon = batch
            .column(7)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(taxon.value(0), 9606);
        assert!(taxon.is_null(1));
    }

    #[tokio::test]
    async fn strings_from_column_extracts_values() {
        let session = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, true)]));
        let ids = StringArray::from(vec![Some("INS"), None, Some("GCG")]);
        let batch = RecordBatch::try_new(schema, vec![Arc::new(ids) as Arc<dyn Array>]).unwrap();
        let df = session.read_batch(batch).unwrap();

        let values = strings_from_column(&df, "id").await.unwrap();
        assert_eq!(values, vec!["INS", "GCG"]);

        assert!(strings_from_column(&df, "missing").await.is_err());
    }

    #[tokio::test]
    async fn missing_column_and_spec_errors() {
        let mut node = UniprotIdmapNode {
            meta: port_layout(),
            spec: UniprotIdmapSpec::default(),
        };
        let ctx = ctx();
        let reporter = dag_core::dag::node_event::NodeReporter::noop();

        // Neither ids nor input → Schedule error.
        let err = node.execute(&ctx, &[], &reporter).await.unwrap_err();
        assert!(matches!(err, DagError::Schedule(_)));
    }
}
