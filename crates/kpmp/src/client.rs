use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::error::{KpmpError, Result};
use crate::types::{
    AtlasSummary, ParticipantDataTypes, ParticipantRecord, ParticipantRepository, SummaryData,
};

pub const DEFAULT_ENDPOINT: &str = "https://atlas.kpmp.org/graphql";

#[derive(Debug, Clone)]
pub struct KpmpClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for KpmpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl KpmpClient {
    pub fn new() -> Self {
        Self::with_endpoint(
            std::env::var("ENDPOINT_KPMP_GRAPHQL").unwrap_or_else(|_| DEFAULT_ENDPOINT.into()),
        )
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-kpmp-sdk/0.1 (+https://atlas.kpmp.org)")
                .build()
                .expect("failed to build KPMP HTTP client"),
            endpoint: endpoint.into(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn summary(&self) -> Result<SummaryData> {
        self.query::<SummaryData>(
            r#"
            query {
                getDataTypeSummaryInformation2025 {
                    omicsType dataType dataTypeShort
                    hrtCount akiCount ckdCount dmrCount totalCount participantCount
                }
                getEnrollmentCategorySummaryData {
                    hrtCount akiCount ckdCount dmrCount
                }
            }
            "#,
            None,
        )
        .await
    }

    pub async fn atlas_summary(&self) -> Result<AtlasSummary> {
        self.query::<AtlasSummary>(
            r#"
            query {
                getAtlasSummaryRows {
                    totalFiles
                    summaryRows {
                        omicsType hrtCount akiCount ckdCount dmrCount totalCount
                        linkInformation { linkType linkValue }
                    }
                }
            }
            "#,
            None,
        )
        .await
    }

    pub async fn participant(&self, redcap_id: &str) -> Result<ParticipantRecord> {
        validate_redcap_id(redcap_id)?;
        self.query::<ParticipantRecord>(
            r#"
            query Participant($redcapId: String!) {
                participantSummaryDataset(redcapId: $redcapId) {
                    participantId redcapId enrollmentCategory adjudicatedCategory
                }
                getParticipantClinicalDataset(redcapId: $redcapId) {
                    participantId participantClinicalId kdigoStage baselineEgfr proteinuria
                    a1c albuminuria diabetesHistory diabetesDuration hypertensionHistory
                    hypertensionDuration onRaasBlockade race ageBinned sex sampleType
                    tissueSource protocol
                }
            }
            "#,
            Some(json!({"redcapId": redcap_id.trim()})),
        )
        .await
    }

    pub async fn participant_data_types(&self, redcap_id: &str) -> Result<ParticipantDataTypes> {
        validate_redcap_id(redcap_id)?;
        let value = self
            .query::<serde_json::Value>(
                r#"
            query ParticipantDataTypes($redcapId: String!) {
                getDataTypeInformationByParticipant2025(redcapId: $redcapId) {
                    spatialViewerDataTypes { dataType count isAggregatedData }
                    explorerDataTypes { dataType count isAggregatedData }
                }
            }
            "#,
                Some(json!({"redcapId": redcap_id.trim()})),
            )
            .await?;
        serde_json::from_value(value["getDataTypeInformationByParticipant2025"].clone())
            .map_err(Into::into)
    }

    pub async fn participant_repository(&self, redcap_id: &str) -> Result<ParticipantRepository> {
        validate_redcap_id(redcap_id)?;
        self.query::<ParticipantRepository>(
            r#"
            query ParticipantRepository($redcapId: String!) {
                getRepoDataTypeInformationByParticipant(redcapId: $redcapId) {
                    repositoryDataTypes {
                        dataType count linkInformation { linkType linkValue }
                    }
                }
                getTotalParticipantFilesCount(redcapId: $redcapId) {
                    dataType count linkInformation { linkType linkValue }
                }
            }
            "#,
            Some(json!({"redcapId": redcap_id.trim()})),
        )
        .await
    }

    async fn query<T>(&self, query: &str, variables: Option<serde_json::Value>) -> Result<T>
    where
        T: DeserializeOwned,
    {
        #[derive(Deserialize)]
        struct GraphqlResponse<T> {
            data: Option<T>,
            errors: Option<Vec<GraphqlError>>,
        }

        #[derive(Deserialize)]
        struct GraphqlError {
            message: String,
        }

        let response = self
            .http
            .post(&self.endpoint)
            .json(&json!({"query": query, "variables": variables.unwrap_or(json!({}))}))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(KpmpError::Status {
                status: status.as_u16(),
                message: body,
            });
        }
        let response: GraphqlResponse<T> = serde_json::from_str(&body)?;
        if let Some(errors) = response.errors {
            let message = errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(KpmpError::Graphql(message));
        }
        response
            .data
            .ok_or_else(|| KpmpError::Graphql("empty response".into()))
    }
}

fn validate_redcap_id(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KpmpError::Param("redcap_id must not be empty".into()));
    }
    Ok(())
}
