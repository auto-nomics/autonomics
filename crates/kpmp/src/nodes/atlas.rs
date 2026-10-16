//! Nodes over the KPMP Atlas GraphQL API.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::KpmpClient;
use crate::nodes::util::{
    bool_array, dataframe_output, dataframe_port, i64, i64_array, request_error, schedule_error,
    str_array, utf8,
};
use crate::types::{
    DataTypeSummary, ParticipantClinical, ParticipantDataType, ParticipantRepositoryRow,
    ParticipantSummary, RepositorySummaryRow,
};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_kpmp_factory {
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
pub struct KpmpDataTypesSpec {
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Clone)]
pub struct KpmpDataTypesNode {
    meta: NodePorts,
    spec: KpmpDataTypesSpec,
}

pub struct KpmpDataTypesNodeFactory;

impl_kpmp_factory!(
    KpmpDataTypesNodeFactory,
    KpmpDataTypesNode,
    KpmpDataTypesSpec,
    "source_kpmp_data_types",
    "Lists KPMP explorer data types and participant counts.",
    "Queries KPMP Atlas `getDataTypeSummaryInformation2025`. Output includes \
     scRNA-seq, snRNA-seq, regional transcriptomics, and regional proteomics \
     participant counts by healthy-reference, AKI, CKD, and DM-R categories."
);

#[async_trait]
impl DagNode for KpmpDataTypesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kpmp_data_types"
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
        let summary = KpmpClient::new()
            .summary()
            .await
            .map_err(|error| request_error(error, "source_kpmp_data_types"))?;
        let mut rows = summary.data_types;
        if let Some(query) = nonempty(&self.spec.query) {
            let query = query.to_ascii_lowercase();
            rows.retain(|row| {
                format!(
                    "{} {} {}",
                    row.omics_type, row.data_type, row.data_type_short
                )
                .to_ascii_lowercase()
                .contains(&query)
            });
        }
        let batch = build_data_type_batch(&rows)?;
        dataframe_output(ctx, batch, "source_kpmp_data_types")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct KpmpRepositorySpec {
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Clone)]
pub struct KpmpRepositoryNode {
    meta: NodePorts,
    spec: KpmpRepositorySpec,
}

pub struct KpmpRepositoryNodeFactory;

impl_kpmp_factory!(
    KpmpRepositoryNodeFactory,
    KpmpRepositoryNode,
    KpmpRepositorySpec,
    "source_kpmp_repository",
    "Lists KPMP repository experimental-strategy file counts.",
    "Queries KPMP Atlas `getAtlasSummaryRows`. Output includes the total \
     repository file count and file counts by experimental strategy across \
     healthy-reference, AKI, CKD, and DM-R cohorts, plus repository link metadata."
);

#[async_trait]
impl DagNode for KpmpRepositoryNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kpmp_repository"
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
        let summary = KpmpClient::new()
            .atlas_summary()
            .await
            .map_err(|error| request_error(error, "source_kpmp_repository"))?;
        let mut rows = summary.result.summary_rows;
        if let Some(query) = nonempty(&self.spec.query) {
            let query = query.to_ascii_lowercase();
            rows.retain(|row| {
                format!(
                    "{} {} {}",
                    row.omics_type, row.link_information.link_type, row.link_information.link_value
                )
                .to_ascii_lowercase()
                .contains(&query)
            });
        }
        let batch = build_repository_batch(&rows, summary.result.total_files)?;
        dataframe_output(ctx, batch, "source_kpmp_repository")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct KpmpParticipantSpec {
    pub redcap_id: String,
}

#[derive(Clone)]
pub struct KpmpParticipantNode {
    meta: NodePorts,
    spec: KpmpParticipantSpec,
}

pub struct KpmpParticipantNodeFactory;

impl_kpmp_factory!(
    KpmpParticipantNodeFactory,
    KpmpParticipantNode,
    KpmpParticipantSpec,
    "source_kpmp_participant",
    "Fetches one KPMP participant and clinical sample record.",
    "Resolves a KPMP REDCap ID through `participantSummaryDataset` and \
     `getParticipantClinicalDataset`. Output includes participant/enrollment \
     metadata, clinical measurements, biosample type, tissue source, and protocol."
);

#[async_trait]
impl DagNode for KpmpParticipantNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kpmp_participant"
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
        let record = KpmpClient::new()
            .participant(&self.spec.redcap_id)
            .await
            .map_err(|error| request_error(error, "source_kpmp_participant"))?;
        let batch = build_participant_batch(&record.summary, &record.clinical)?;
        dataframe_output(ctx, batch, "source_kpmp_participant")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct KpmpParticipantDatasetsSpec {
    pub redcap_id: String,
    /// `explorer`, `spatial_viewer`, or omit for both.
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Clone)]
pub struct KpmpParticipantDatasetsNode {
    meta: NodePorts,
    spec: KpmpParticipantDatasetsSpec,
}

pub struct KpmpParticipantDatasetsNodeFactory;

impl_kpmp_factory!(
    KpmpParticipantDatasetsNodeFactory,
    KpmpParticipantDatasetsNode,
    KpmpParticipantDatasetsSpec,
    "source_kpmp_participant_datasets",
    "Lists KPMP Explorer and Spatial Viewer datasets for a participant.",
    "Queries `getDataTypeInformationByParticipant2025`. Rows describe available \
     data types, counts, and whether the data is aggregated into the Explorer \
     reference rather than participant-level."
);

#[async_trait]
impl DagNode for KpmpParticipantDatasetsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kpmp_participant_datasets"
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
        let response = KpmpClient::new()
            .participant_data_types(&self.spec.redcap_id)
            .await
            .map_err(|error| request_error(error, "source_kpmp_participant_datasets"))?;
        let category = self
            .spec
            .category
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut rows = Vec::new();
        if category.is_none_or(|value| value.eq_ignore_ascii_case("spatial_viewer")) {
            rows.extend(
                response
                    .spatial_viewer
                    .into_iter()
                    .map(|row| ("spatial_viewer", row)),
            );
        }
        if category.is_none_or(|value| value.eq_ignore_ascii_case("explorer")) {
            rows.extend(response.explorer.into_iter().map(|row| ("explorer", row)));
        }
        let batch = build_participant_data_type_batch(
            self.spec.redcap_id.trim(),
            &rows
                .iter()
                .map(|(category, row)| (*category, row.clone()))
                .collect::<Vec<_>>(),
        )?;
        dataframe_output(ctx, batch, "source_kpmp_participant_datasets")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct KpmpParticipantRepositorySpec {
    pub redcap_id: String,
    #[serde(default)]
    pub data_type: Option<String>,
}

#[derive(Clone)]
pub struct KpmpParticipantRepositoryNode {
    meta: NodePorts,
    spec: KpmpParticipantRepositorySpec,
}

pub struct KpmpParticipantRepositoryNodeFactory;

impl_kpmp_factory!(
    KpmpParticipantRepositoryNodeFactory,
    KpmpParticipantRepositoryNode,
    KpmpParticipantRepositorySpec,
    "source_kpmp_participant_repository",
    "Lists KPMP repository files for one participant without downloading them.",
    "Queries KPMP repository data types and the participant's total file count. \
     Rows include data type, file count, and repository link metadata; file \
     transfer remains a separate workflow."
);

#[async_trait]
impl DagNode for KpmpParticipantRepositoryNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_kpmp_participant_repository"
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
        let response = KpmpClient::new()
            .participant_repository(&self.spec.redcap_id)
            .await
            .map_err(|error| request_error(error, "source_kpmp_participant_repository"))?;
        let mut rows = response.repository.rows;
        if let Some(data_type) = nonempty(&self.spec.data_type) {
            let data_type = data_type.to_ascii_lowercase();
            rows.retain(|row| row.data_type.to_ascii_lowercase().contains(&data_type));
        }
        let batch = build_participant_repository_batch(
            self.spec.redcap_id.trim(),
            &rows,
            response.total.count,
        )?;
        dataframe_output(ctx, batch, "source_kpmp_participant_repository")
    }
}

fn nonempty(value: &Option<String>) -> Option<&str> {
    let value = value.as_deref()?.trim();
    (!value.is_empty()).then_some(value)
}

fn build_data_type_batch(rows: &[DataTypeSummary]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("omics_type"),
        utf8("data_type"),
        utf8("data_type_short"),
        i64("healthy_reference"),
        i64("aki"),
        i64("ckd"),
        i64("dmr"),
        i64("total_count"),
        i64("participant_count"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(
                rows.iter()
                    .map(|row| Some(row.omics_type.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.data_type.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.data_type_short.clone()))
                    .collect(),
            ),
            i64_array(rows.iter().map(|row| row.hrt_count).collect()),
            i64_array(rows.iter().map(|row| row.aki_count).collect()),
            i64_array(rows.iter().map(|row| row.ckd_count).collect()),
            i64_array(rows.iter().map(|row| row.dmr_count).collect()),
            i64_array(rows.iter().map(|row| row.total_count).collect()),
            i64_array(rows.iter().map(|row| row.participant_count).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_kpmp_data_types", error))
}

fn build_repository_batch(
    rows: &[RepositorySummaryRow],
    total_files: i64,
) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        i64("total_files"),
        utf8("omics_type"),
        i64("healthy_reference"),
        i64("aki"),
        i64("ckd"),
        i64("dmr"),
        i64("total_count"),
        utf8("link_type"),
        utf8("link_value"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            i64_array(vec![total_files; rows.len()]),
            str_array(
                rows.iter()
                    .map(|row| Some(row.omics_type.clone()))
                    .collect(),
            ),
            i64_array(rows.iter().map(|row| row.hrt_count).collect()),
            i64_array(rows.iter().map(|row| row.aki_count).collect()),
            i64_array(rows.iter().map(|row| row.ckd_count).collect()),
            i64_array(rows.iter().map(|row| row.dmr_count).collect()),
            i64_array(rows.iter().map(|row| row.total_count).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.link_information.link_type.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.link_information.link_value.clone()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_kpmp_repository", error))
}

fn build_participant_batch(
    summary: &ParticipantSummary,
    clinical: &ParticipantClinical,
) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("participant_id"),
        utf8("redcap_id"),
        utf8("enrollment_category"),
        utf8("adjudicated_category"),
        utf8("kdigo_stage"),
        utf8("baseline_egfr"),
        utf8("proteinuria"),
        utf8("a1c"),
        utf8("albuminuria"),
        utf8("diabetes_history"),
        utf8("diabetes_duration"),
        utf8("hypertension_history"),
        utf8("hypertension_duration"),
        utf8("on_raas_blockade"),
        utf8("race"),
        utf8("age_binned"),
        utf8("sex"),
        utf8("sample_type"),
        utf8("tissue_source"),
        utf8("protocol"),
    ]));
    let values = vec![
        Some(summary.participant_id.clone()),
        Some(summary.redcap_id.clone()),
        Some(summary.enrollment_category.clone()),
        Some(summary.adjudicated_category.clone()),
        Some(clinical.kdigo_stage.clone()),
        Some(clinical.baseline_egfr.clone()),
        Some(clinical.proteinuria.clone()),
        Some(clinical.a1c.clone()),
        Some(clinical.albuminuria.clone()),
        Some(clinical.diabetes_history.clone()),
        Some(clinical.diabetes_duration.clone()),
        Some(clinical.hypertension_history.clone()),
        Some(clinical.hypertension_duration.clone()),
        Some(clinical.on_raas_blockade.clone()),
        Some(clinical.race.clone()),
        Some(clinical.age_binned.clone()),
        Some(clinical.sex.clone()),
        Some(clinical.sample_type.clone()),
        Some(clinical.tissue_source.clone()),
        Some(clinical.protocol.clone()),
    ];
    let columns = (0..20)
        .map(|index| str_array(vec![values[index].clone()]))
        .collect();
    RecordBatch::try_new(schema, columns)
        .map_err(|error| schedule_error("source_kpmp_participant", error))
}

fn build_participant_data_type_batch(
    redcap_id: &str,
    rows: &[(&'static str, ParticipantDataType)],
) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("redcap_id"),
        utf8("category"),
        utf8("data_type"),
        i64("count"),
        arrow_schema::Field::new("is_aggregated_data", arrow_schema::DataType::Boolean, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(redcap_id.to_owned()); rows.len()]),
            str_array(rows.iter().map(|row| Some(row.0.to_owned())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.1.data_type.clone()))
                    .collect(),
            ),
            i64_array(rows.iter().map(|row| row.1.count).collect()),
            bool_array(rows.iter().map(|row| row.1.is_aggregated_data).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_kpmp_participant_datasets", error))
}

fn build_participant_repository_batch(
    redcap_id: &str,
    rows: &[ParticipantRepositoryRow],
    total_count: i64,
) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("redcap_id"),
        utf8("data_type"),
        i64("file_count"),
        i64("participant_total_files"),
        utf8("link_type"),
        utf8("link_value"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(redcap_id.to_owned()); rows.len()]),
            str_array(rows.iter().map(|row| Some(row.data_type.clone())).collect()),
            i64_array(rows.iter().map(|row| row.count).collect()),
            i64_array(vec![total_count; rows.len()]),
            str_array(
                rows.iter()
                    .map(|row| Some(row.link_information.link_type.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.link_information.link_value.clone()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_kpmp_participant_repository", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_batches() {
        let data_type = DataTypeSummary::default();
        assert_eq!(
            build_data_type_batch(&[data_type]).unwrap().num_columns(),
            9
        );
        let repository = RepositorySummaryRow::default();
        assert_eq!(
            build_repository_batch(&[repository], 44)
                .unwrap()
                .num_columns(),
            9
        );
        let summary = ParticipantSummary::default();
        let clinical = ParticipantClinical::default();
        assert_eq!(
            build_participant_batch(&summary, &clinical)
                .unwrap()
                .num_columns(),
            20
        );
        let participant_data_type = ParticipantDataType::default();
        assert_eq!(
            build_participant_data_type_batch("164-1", &[("explorer", participant_data_type)])
                .unwrap()
                .num_columns(),
            5
        );
        let repository_row = ParticipantRepositoryRow::default();
        assert_eq!(
            build_participant_repository_batch("164-1", &[repository_row], 44)
                .unwrap()
                .num_columns(),
            6
        );
    }
}
