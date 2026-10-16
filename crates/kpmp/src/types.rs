use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataTypeSummary {
    #[serde(default)]
    pub omics_type: String,
    pub data_type: String,
    #[serde(rename = "dataTypeShort")]
    pub data_type_short: String,
    pub hrt_count: i64,
    pub aki_count: i64,
    pub ckd_count: i64,
    pub dmr_count: i64,
    pub total_count: i64,
    pub participant_count: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollmentSummary {
    pub hrt_count: i64,
    pub aki_count: i64,
    pub ckd_count: i64,
    pub dmr_count: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SummaryData {
    #[serde(rename = "getDataTypeSummaryInformation2025")]
    pub data_types: Vec<DataTypeSummary>,
    #[serde(rename = "getEnrollmentCategorySummaryData")]
    pub enrollment: Vec<EnrollmentSummary>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryLink {
    pub link_type: String,
    pub link_value: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySummaryRow {
    pub omics_type: String,
    pub hrt_count: i64,
    pub aki_count: i64,
    pub ckd_count: i64,
    pub dmr_count: i64,
    pub total_count: i64,
    pub link_information: RepositoryLink,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AtlasSummary {
    #[serde(rename = "getAtlasSummaryRows")]
    pub result: AtlasSummaryResult,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtlasSummaryResult {
    pub total_files: i64,
    pub summary_rows: Vec<RepositorySummaryRow>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantSummary {
    pub participant_id: String,
    pub redcap_id: String,
    pub enrollment_category: String,
    pub adjudicated_category: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantClinical {
    pub participant_id: String,
    pub participant_clinical_id: String,
    pub kdigo_stage: String,
    pub baseline_egfr: String,
    pub proteinuria: String,
    pub a1c: String,
    pub albuminuria: String,
    pub diabetes_history: String,
    pub diabetes_duration: String,
    pub hypertension_history: String,
    pub hypertension_duration: String,
    pub on_raas_blockade: String,
    pub race: String,
    pub age_binned: String,
    pub sex: String,
    pub sample_type: String,
    pub tissue_source: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ParticipantRecord {
    #[serde(rename = "participantSummaryDataset")]
    pub summary: ParticipantSummary,
    #[serde(rename = "getParticipantClinicalDataset")]
    pub clinical: ParticipantClinical,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantDataType {
    pub data_type: String,
    pub count: i64,
    pub is_aggregated_data: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ParticipantDataTypes {
    #[serde(rename = "spatialViewerDataTypes")]
    #[serde(default)]
    pub spatial_viewer: Vec<ParticipantDataType>,
    #[serde(rename = "explorerDataTypes")]
    #[serde(default)]
    pub explorer: Vec<ParticipantDataType>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantRepositoryRow {
    pub data_type: String,
    pub count: i64,
    pub link_information: RepositoryLink,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ParticipantRepository {
    #[serde(rename = "getRepoDataTypeInformationByParticipant")]
    pub repository: ParticipantRepositoryGroups,
    #[serde(rename = "getTotalParticipantFilesCount")]
    pub total: ParticipantRepositoryRow,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ParticipantRepositoryGroups {
    #[serde(rename = "repositoryDataTypes")]
    pub rows: Vec<ParticipantRepositoryRow>,
}
