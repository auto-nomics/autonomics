use crate::error::{ClinicalTrialsError, Result};
use crate::types::Study;

pub const DEFAULT_ENDPOINT: &str = "https://clinicaltrials.gov/api/v2";

#[derive(Debug, Clone)]
pub struct ClinicalTrialsClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for ClinicalTrialsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ClinicalTrialsClient {
    pub fn new() -> Self {
        let endpoint = std::env::var("ENDPOINT_CLINICALTRIALS_URL")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        Self::with_endpoint(endpoint)
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("clinicaltrials-rs-sdk/0.1 (+https://clinicaltrials.gov)")
                .build()
                .expect("failed to build HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn study(&self, nct_id: &str) -> Result<Study> {
        normalize_nct_id(nct_id)?;
        let url = format!("{}/studies/{}", self.endpoint, nct_id.trim());
        let response = self.http.get(&url).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        Ok(serde_json::from_str::<Study>(&body)?)
    }
}

pub(crate) fn normalize_nct_id(nct_id: &str) -> Result<&str> {
    let nct_id = nct_id.trim();
    if nct_id.len() == 11
        && nct_id[0..3].eq_ignore_ascii_case("NCT")
        && nct_id[3..].bytes().all(|byte| byte.is_ascii_digit())
    {
        Ok(nct_id)
    } else {
        Err(ClinicalTrialsError::InvalidNctId(nct_id.to_owned()))
    }
}

fn api_error(status: reqwest::StatusCode, body: &str) -> ClinicalTrialsError {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .or_else(|| value.get("error"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                status.to_string()
            } else {
                trimmed.to_owned()
            }
        });
    ClinicalTrialsError::Api {
        status: status.as_u16(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_nct_identifiers() {
        assert_eq!(normalize_nct_id(" nct04280705 ").unwrap(), "nct04280705");
        assert!(normalize_nct_id("NCT042807").is_err());
        assert!(normalize_nct_id("NCT0428070A").is_err());
    }

    #[test]
    fn parses_protocol_sections() {
        let value = serde_json::json!({
            "protocolSection": {
                "identificationModule": {"nctId": "NCT04280705", "briefTitle": "Example"},
                "statusModule": {"overallStatus": "COMPLETED"},
                "sponsorCollaboratorsModule": {"leadSponsor": {"name": "Example Sponsor"}},
                "conditionsModule": {"conditions": ["Example condition"]},
                "designModule": {"studyType": "INTERVENTIONAL", "phases": ["PHASE3"]}
            }
        });
        let study: Study = serde_json::from_value(value).unwrap();
        assert_eq!(
            study
                .protocol_section
                .identification_module
                .nct_id
                .as_deref(),
            Some("NCT04280705")
        );
    }
}
