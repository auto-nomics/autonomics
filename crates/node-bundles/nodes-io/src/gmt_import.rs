//! Gene Matrix Transposed (GMT) file source node.

use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::prelude::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

pub const GMT_IMPORT_KIND: &str = "gmt_import";

const PATHWAY_NAME: &str = "pathway_name";
const DESCRIPTION: &str = "description";
const GENE: &str = "gene";

fn default_min_set_size() -> usize {
    3
}

fn default_max_set_size() -> usize {
    usize::MAX
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct GmtImportSpec {
    /// GMT address in the runtime VFS, used when no upstream File is connected.
    pub path: Option<String>,
    #[serde(default = "default_min_set_size")]
    pub min_set_size: usize,
    #[serde(default = "default_max_set_size")]
    pub max_set_size: usize,
    #[serde(default)]
    pub uppercase_genes: bool,
}

#[derive(Debug, Error)]
pub enum GmtImportError {
    #[error("invalid GMT at line {line}: {message}")]
    Invalid { line: usize, message: String },
    #[error("cannot read GMT `{path}`: {source}")]
    Read {
        path: String,
        source: vfs::opendal::Error,
    },
    #[error("GMT `{path}` is not valid UTF-8")]
    Encoding { path: String },
    #[error("gmt_import requires an upstream file or a fallback path")]
    MissingInput,
}

impl From<GmtImportError> for DagError {
    fn from(error: GmtImportError) -> Self {
        DagError::Schedule(error.to_string())
    }
}

#[derive(Debug, Clone)]
struct GeneSet {
    name: String,
    description: String,
    genes: Vec<String>,
}

#[derive(Clone)]
pub struct GmtImportNode {
    meta: NodePorts,
    spec: GmtImportSpec,
}

pub struct GmtImportNodeFactory;

fn output_port() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::File)
        .add_output_port(None)
}

fn output_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(PATHWAY_NAME, DataType::Utf8, false),
        Field::new(DESCRIPTION, DataType::Utf8, false),
        Field::new(GENE, DataType::Utf8, false),
    ]))
}

impl NodeFactory for GmtImportNodeFactory {
    fn kind(&self) -> &'static str {
        GMT_IMPORT_KIND
    }

    fn desc(&self) -> &'static str {
        "Reads a GMT file into a long-format gene-set DataFrame."
    }

    fn doc(&self) -> &'static str {
        "Reads tab-separated GMT content through the runtime VFS. Each output \
        row is pathway_name, description, gene and preserves source order. \
        Gene sets outside min_set_size..=max_set_size are omitted with a \
        diagnostic rather than failing the node."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GmtImportSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: GmtImportSpec = serde_json::from_value(spec)?;
        validate_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(Box::new(GmtImportNode {
            meta: output_port(),
            spec,
        }))
    }
}

fn validate_spec(spec: &GmtImportSpec) -> Result<(), String> {
    if spec.min_set_size == 0 {
        return Err("min_set_size must be greater than zero".into());
    }
    if spec.max_set_size < spec.min_set_size {
        return Err("max_set_size must be at least min_set_size".into());
    }
    Ok(())
}

async fn read_gmt(ctx: &NodeCtx, path: &str) -> Result<String, GmtImportError> {
    let storage = ctx.opendal.as_ref().ok_or_else(|| GmtImportError::Read {
        path: path.to_string(),
        source: vfs::opendal::Error::new(
            vfs::opendal::ErrorKind::Unexpected,
            "gmt_import requires a registered runtime VFS",
        ),
    })?;
    let operator = storage.resolve(path);
    let key = storage.resolve_path(path);
    let bytes = operator
        .read(&key)
        .await
        .map_err(|source| GmtImportError::Read {
            path: path.to_string(),
            source,
        })?;
    String::from_utf8(bytes.to_vec()).map_err(|_| GmtImportError::Encoding {
        path: path.to_string(),
    })
}

fn parse_gmt(
    content: &str,
    spec: &GmtImportSpec,
    reporter: &NodeReporter,
) -> Result<Vec<GeneSet>, GmtImportError> {
    let mut sets = Vec::new();
    let mut omitted = Vec::new();
    for (offset, line) in content.lines().enumerate() {
        let line_number = offset + 1;
        if line.trim().is_empty() {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        let name = fields.first().copied().unwrap_or_default();
        if name.trim().is_empty() {
            return Err(GmtImportError::Invalid {
                line: line_number,
                message: "set name is empty".into(),
            });
        }
        let description = fields.get(1).copied().unwrap_or_default();
        if description.trim().is_empty() {
            return Err(GmtImportError::Invalid {
                line: line_number,
                message: "description is missing or empty".into(),
            });
        }
        if fields.len() < 3 || fields[2..].iter().any(|gene| gene.is_empty()) {
            return Err(GmtImportError::Invalid {
                line: line_number,
                message: "one or more gene fields are missing or empty".into(),
            });
        }

        let genes = fields[2..]
            .iter()
            .map(|gene| {
                if spec.uppercase_genes {
                    gene.to_ascii_uppercase()
                } else {
                    gene.to_string()
                }
            })
            .collect::<Vec<_>>();
        if genes.len() < spec.min_set_size || genes.len() > spec.max_set_size {
            omitted.push(format!("{} ({})", name, genes.len()));
            continue;
        }
        sets.push(GeneSet {
            name: name.to_string(),
            description: description.to_string(),
            genes,
        });
    }

    if !omitted.is_empty() {
        reporter.info(format!(
            "filtered {} gene set(s) outside size range [{}, {}]: {}",
            omitted.len(),
            spec.min_set_size,
            spec.max_set_size,
            omitted.join(", ")
        ));
    }
    Ok(sets)
}

fn output_batch(sets: Vec<GeneSet>) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let mut names = Vec::with_capacity(sets.iter().map(|set| set.genes.len()).sum::<usize>());
    let mut descriptions = Vec::with_capacity(names.capacity());
    let mut genes = Vec::with_capacity(names.capacity());
    for set in sets {
        for gene in set.genes {
            names.push(set.name.clone());
            descriptions.push(set.description.clone());
            genes.push(gene);
        }
    }
    RecordBatch::try_new(
        output_schema(),
        vec![
            Arc::new(StringArray::from(names)),
            Arc::new(StringArray::from(descriptions)),
            Arc::new(StringArray::from(genes)),
        ],
    )
}

#[async_trait]
impl DagNode for GmtImportNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            spec: self.spec.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        GMT_IMPORT_KIND
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
        let path = inputs
            .first()
            .and_then(|input| input.file_value().ok())
            .map(|file| file.path.clone())
            .or_else(|| self.spec.path.clone())
            .ok_or(GmtImportError::MissingInput)?;
        let content = read_gmt(ctx, &path).await?;
        let sets = parse_gmt(&content, &self.spec, reporter)?;
        let batch = output_batch(sets).map_err(|error| GmtImportError::Invalid {
            line: 0,
            message: format!("cannot build gene-set table: {error}"),
        })?;
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|error| DagError::Schedule(format!("failed to read GMT output: {error}")))?;

        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use vfs::OpendalFileStorage;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("fixtures/msigdb")
            .join(name)
    }

    fn test_spec() -> GmtImportSpec {
        GmtImportSpec {
            path: None,
            min_set_size: 3,
            max_set_size: usize::MAX,
            uppercase_genes: false,
        }
    }

    #[test]
    fn parses_minimal_fixture_in_source_order() {
        let content = std::fs::read_to_string(fixture("hallmark_minimal.gmt")).unwrap();
        let reporter = NodeReporter::noop();
        let sets = parse_gmt(&content, &test_spec(), &reporter).unwrap();

        assert_eq!(sets.len(), 3);
        assert_eq!(
            sets.iter().map(|set| set.genes.len()).collect::<Vec<_>>(),
            vec![10, 10, 10]
        );
        assert_eq!(sets[0].name, "HALLMARK_ADIPOGENESIS");
        assert_eq!(
            sets[0].description,
            "https://www.gsea-msigdb.org/gsea/msigdb/human/geneset/HALLMARK_ADIPOGENESIS"
        );
        assert_eq!(sets[0].genes.first().unwrap(), "ABCA1");
    }

    #[test]
    fn validates_official_hallmark_manifest_digest_and_counts() {
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture("manifest.json")).unwrap())
                .unwrap();
        let expected = manifest["files"]["h.all.v2025.1.Hs.symbols.gmt"]["sha256"]
            .as_str()
            .unwrap();
        let bytes = std::fs::read(fixture("h.all.v2025.1.Hs.symbols.gmt")).unwrap();
        let digest = Sha256::new().chain_update(&bytes).finalize();
        let actual = digest
            .iter()
            .fold(String::new(), |out, byte| format!("{out}{byte:02x}"));
        assert_eq!(actual, expected);

        let content = std::str::from_utf8(&bytes).unwrap();
        let sets = parse_gmt(content, &test_spec(), &NodeReporter::noop()).unwrap();
        assert_eq!(sets.len(), 50);
        assert_eq!(sets.iter().map(|set| set.genes.len()).sum::<usize>(), 7322);
    }

    #[test]
    fn reports_sets_filtered_by_size() {
        let content = "small\tdescription\tA\tB\nlarge\tdescription\tA\tB\tC\n";
        let spec = GmtImportSpec {
            min_set_size: 3,
            max_set_size: 3,
            ..test_spec()
        };
        let sets = parse_gmt(content, &spec, &NodeReporter::noop()).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "large");
    }

    #[test]
    fn malformed_lines_report_line_numbers() {
        let content = "good\tdescription\tA\tB\n\nbroken\t\n";
        let error = parse_gmt(content, &test_spec(), &NodeReporter::noop()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invalid GMT at line 3: description is missing or empty")
        );
    }

    #[tokio::test]
    async fn reads_spec_path_through_vfs() {
        let (session, storage) = OpendalFileStorage::new_temp().register_to_ctx();
        storage
            .write_bytes("/pathways.gmt", b"set\tdescription\tA\tB\tC".to_vec())
            .await
            .unwrap();
        let node_ctx = NodeCtx::new(session.runtime_env().clone(), Some(storage));
        let mut node = GmtImportNode {
            meta: output_port(),
            spec: GmtImportSpec {
                path: Some("/pathways.gmt".into()),
                ..test_spec()
            },
        };

        let outputs = node
            .execute(&node_ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let batches = outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(batches[0].num_rows(), 3);
        assert_eq!(batches[0].schema(), output_schema());
    }
}
