//! Nodes over HTAN Portal's public read-only ClickHouse query layer.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::HtanClient;
use crate::nodes::util::{
    dataframe_output, dataframe_port, request_error, schedule_error, str_array, u64, u64_array,
    utf8,
};
use crate::types::{Atlas, DatasetFile, DatasetSummary, FileFilter};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_htan_factory {
    ($factory:ident, $node:ident, $spec:ident, $kind:literal, $description:literal, $documentation:literal) => {
        impl NodeFactory for $factory {
            fn kind(&self) -> &'static str {
                $kind
            }

            fn desc(&self) -> &'static str {
                $description
            }

            fn doc(&self) -> &'static str {
                $documentation
            }

            fn spec_schema(&self) -> schemars::Schema {
                schema_for!($spec)
            }

            fn ports(&self) -> NodePorts {
                dataframe_port()
            }

            fn build(
                &self,
                spec: serde_json::Value,
                _node_ctx: NodeCtx,
            ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
                let spec = serde_json::from_value::<$spec>(spec)?;
                Ok(Box::new($node {
                    meta: dataframe_port(),
                    spec,
                }))
            }
        }
    };
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HtanAtlasesSpec {}

#[derive(Clone)]
pub struct HtanAtlasesNode {
    meta: NodePorts,
    #[allow(dead_code)]
    spec: HtanAtlasesSpec,
}

pub struct HtanAtlasesNodeFactory;

impl_htan_factory!(
    HtanAtlasesNodeFactory,
    HtanAtlasesNode,
    HtanAtlasesSpec,
    "source_htan_atlases",
    "Lists Human Tumor Atlas Network atlases and file counts.",
    "Queries HTAN Portal's public read-only ClickHouse `atlases` and `files` \
     tables. Output includes HTAN ID, atlas name, title, lead institution, \
     biospecimen/case counts, file rows, and distinct data-file counts."
);

#[async_trait]
impl DagNode for HtanAtlasesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_htan_atlases"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let atlases = HtanClient::new()
            .atlases()
            .await
            .map_err(|error| request_error(error, "source_htan_atlases"))?;
        let batch = build_atlas_batch(&atlases)?;
        dataframe_output(ctx, batch, "source_htan_atlases")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HtanDatasetsSpec {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub atlas: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub assay: Option<String>,
    #[serde(default)]
    pub organ: Option<String>,
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default)]
    pub file_format: Option<String>,
    #[serde(default)]
    pub release: Option<String>,
    #[serde(default = "default_max")]
    pub max_results: u64,
}

fn default_max() -> u64 {
    100
}

#[derive(Clone)]
pub struct HtanDatasetsNode {
    meta: NodePorts,
    spec: HtanDatasetsSpec,
}

pub struct HtanDatasetsNodeFactory;

impl_htan_factory!(
    HtanDatasetsNodeFactory,
    HtanDatasetsNode,
    HtanDatasetsSpec,
    "source_htan_datasets",
    "Aggregates HTAN files by atlas, assay, level, and organ.",
    "Queries the HTAN Portal `files` table and emits dataset-level counts \
     by atlas, processing level, assay, component, organ, release, and \
     download source. This node returns aggregate metadata only."
);

#[async_trait]
impl DagNode for HtanDatasetsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_htan_datasets"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let filter = self.spec.filter();
        let summaries = HtanClient::new()
            .dataset_summaries(&filter)
            .await
            .map_err(|error| request_error(error, "source_htan_datasets"))?;
        let batch = build_dataset_summary_batch(&summaries)?;
        dataframe_output(ctx, batch, "source_htan_datasets")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct HtanFilesSpec {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub atlas: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub assay: Option<String>,
    #[serde(default)]
    pub organ: Option<String>,
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default)]
    pub file_format: Option<String>,
    #[serde(default)]
    pub release: Option<String>,
    #[serde(default = "default_max")]
    pub max_results: u64,
}

#[derive(Clone)]
pub struct HtanFilesNode {
    meta: NodePorts,
    spec: HtanFilesSpec,
}

pub struct HtanFilesNodeFactory;

impl_htan_factory!(
    HtanFilesNodeFactory,
    HtanFilesNode,
    HtanFilesSpec,
    "source_htan_files",
    "Lists HTAN data-file manifest rows without downloading them.",
    "Queries the HTAN Portal `files` table. Rows include Synapse entity ID, \
     atlas, level, assay, filename, format, HTAN data-file ID, component, \
     organ, release, download source, biospecimen IDs, and publication IDs. \
     Synapse file transfer remains a separate workflow."
);

#[async_trait]
impl DagNode for HtanFilesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_htan_files"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let filter = self.spec.filter();
        let files = HtanClient::new()
            .files(&filter)
            .await
            .map_err(|error| request_error(error, "source_htan_files"))?;
        let batch = build_file_batch(&files)?;
        dataframe_output(ctx, batch, "source_htan_files")
    }
}

trait FileFilterFromSpec {
    fn filter(&self) -> FileFilter;
}

impl FileFilterFromSpec for HtanDatasetsSpec {
    fn filter(&self) -> FileFilter {
        FileFilter {
            query: self.query.clone(),
            atlas: self.atlas.clone(),
            level: self.level.clone(),
            assay: self.assay.clone(),
            organ: self.organ.clone(),
            component: self.component.clone(),
            file_format: self.file_format.clone(),
            release: self.release.clone(),
            max_results: Some(self.max_results),
        }
    }
}

impl FileFilterFromSpec for HtanFilesSpec {
    fn filter(&self) -> FileFilter {
        FileFilter {
            query: self.query.clone(),
            atlas: self.atlas.clone(),
            level: self.level.clone(),
            assay: self.assay.clone(),
            organ: self.organ.clone(),
            component: self.component.clone(),
            file_format: self.file_format.clone(),
            release: self.release.clone(),
            max_results: Some(self.max_results),
        }
    }
}

fn joined(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join("; "))
}

fn json(value: &serde_json::Value) -> Option<String> {
    serde_json::to_string(value).ok()
}

fn build_atlas_batch(rows: &[Atlas]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("htan_id"),
        utf8("htan_name"),
        utf8("title"),
        utf8("lead_institutions"),
        utf8("short_description"),
        u64("biospecimens"),
        u64("cases"),
        u64("file_rows"),
        u64("data_files"),
        utf8("atlas_meta_json"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.htan_id.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.htan_name.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.title.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.lead_institutions.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.short_description.clone()))
                    .collect(),
            ),
            u64_array(rows.iter().map(|row| row.num_biospecimens).collect()),
            u64_array(rows.iter().map(|row| row.num_cases).collect()),
            u64_array(rows.iter().map(|row| row.file_rows).collect()),
            u64_array(rows.iter().map(|row| row.data_files).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.atlas_meta.as_ref().and_then(json))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_htan_atlases", error))
}

fn build_dataset_summary_batch(rows: &[DatasetSummary]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("atlas_id"),
        utf8("atlas_name"),
        utf8("level"),
        utf8("assay"),
        utf8("component"),
        utf8("organ_type"),
        utf8("release_version"),
        utf8("download_source"),
        u64("file_rows"),
        u64("data_files"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.atlasid.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.atlas_name.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.level.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.assay_name.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.component.clone())).collect()),
            str_array(rows.iter().map(|row| joined(&row.organ_type)).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.release_version.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.download_source.clone()))
                    .collect(),
            ),
            u64_array(rows.iter().map(|row| row.file_rows).collect()),
            u64_array(rows.iter().map(|row| row.data_files).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_htan_datasets", error))
}

fn build_file_batch(rows: &[DatasetFile]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("synapse_id"),
        utf8("atlas_id"),
        utf8("atlas_name"),
        utf8("level"),
        utf8("assay"),
        utf8("filename"),
        utf8("file_format"),
        utf8("data_file_id"),
        utf8("component"),
        utf8("organ_type"),
        utf8("release_version"),
        utf8("download_source"),
        utf8("biospecimen_ids"),
        utf8("publication_ids"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(
                rows.iter()
                    .map(|row| Some(row.synapse_id.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.atlasid.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.atlas_name.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.level.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.assay_name.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.filename.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.file_format.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.data_file_id.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.component.clone())).collect()),
            str_array(rows.iter().map(|row| joined(&row.organ_type)).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.release_version.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.download_source.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| joined(&row.biospecimen_ids))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| joined(&row.publication_ids))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_htan_files", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_batches() {
        let atlas = Atlas::default();
        assert_eq!(build_atlas_batch(&[atlas]).unwrap().num_columns(), 10);
        let summary = DatasetSummary::default();
        assert_eq!(
            build_dataset_summary_batch(&[summary])
                .unwrap()
                .num_columns(),
            10
        );
        let file = DatasetFile::default();
        assert_eq!(build_file_batch(&[file]).unwrap().num_columns(), 14);
    }
}
