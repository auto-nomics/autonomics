use serde::de::DeserializeOwned;

use crate::error::{AlphaFoldError, Result};
use crate::types::Prediction;

pub const DEFAULT_ENDPOINT: &str = "https://alphafold.ebi.ac.uk/api";

#[derive(Debug, Clone)]
pub struct AlphaFoldClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for AlphaFoldClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AlphaFoldClient {
    pub fn new() -> Self {
        let endpoint =
            std::env::var("ENDPOINT_ALPHAFOLD_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        Self::with_endpoint(endpoint)
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("alphafold-rs-sdk/0.1 (+https://alphafold.ebi.ac.uk)")
                .build()
                .expect("failed to build HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn prediction(&self, accession: &str) -> Result<Vec<Prediction>> {
        normalize_accession(accession)?;
        let url = format!("{}/prediction/{}", self.endpoint, accession);
        let response = self.http.get(&url).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        Ok(serde_json::from_str(&body)?)
    }
}

pub(crate) fn normalize_accession(accession: &str) -> Result<&str> {
    let accession = accession.trim();
    let valid =
        matches!(accession.len(), 6 | 10) && accession.chars().all(|ch| ch.is_ascii_alphanumeric());
    if valid {
        Ok(accession)
    } else {
        Err(AlphaFoldError::InvalidAccession(accession.to_owned()))
    }
}

fn api_error(status: reqwest::StatusCode, body: &str) -> AlphaFoldError {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .or_else(|| value.get("detail"))
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
    AlphaFoldError::Api {
        status: status.as_u16(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_uniprot_accession_shapes() {
        assert_eq!(normalize_accession(" P01308 ").unwrap(), "P01308");
        assert_eq!(normalize_accession("A0A0B4J2F0").unwrap(), "A0A0B4J2F0");
        assert!(normalize_accession("P01").is_err());
        assert!(normalize_accession("P0130-1").is_err());
    }

    #[test]
    fn parses_live_response_shape() {
        let value = serde_json::json!([{
            "entryId": "AF-P01308-F1",
            "modelEntityId": "AF-P01308-F1",
            "uniprotAccession": "P01308",
            "latestVersion": 6,
            "globalMetricValue": 52.91,
            "cifUrl": "https://example.test/model.cif"
        }]);
        let predictions: Vec<Prediction> = serde_json::from_value(value).unwrap();
        assert_eq!(predictions.len(), 1);
        assert_eq!(predictions[0].latest_version, Some(6));
    }
}
