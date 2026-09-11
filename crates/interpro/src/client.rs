use serde::de::DeserializeOwned;

use crate::error::{InterProError, Result};
use crate::types::EntryResponse;

pub const DEFAULT_ENDPOINT: &str = "https://www.ebi.ac.uk/interpro/api";

#[derive(Debug, Clone)]
pub struct InterProClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for InterProClient {
    fn default() -> Self {
        Self::new()
    }
}

impl InterProClient {
    pub fn new() -> Self {
        let endpoint =
            std::env::var("ENDPOINT_INTERPRO_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        Self::with_endpoint(endpoint)
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("interpro-rs-sdk/0.1 (+https://www.ebi.ac.uk/interpro)")
                .build()
                .expect("failed to build HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn entry(&self, accession: &str) -> Result<EntryResponse> {
        let accession = normalize_accession(accession)?;
        let url = format!("{}/entry/interpro/{accession}/", self.endpoint);
        self.get_json(&url).await
    }

    async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self.http.get(url).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        Ok(serde_json::from_str(&body)?)
    }
}

pub(crate) fn normalize_accession(accession: &str) -> Result<String> {
    let accession = accession.trim();
    if accession.len() == 9
        && accession[0..3].eq_ignore_ascii_case("IPR")
        && accession[3..].bytes().all(|byte| byte.is_ascii_digit())
    {
        Ok(accession.to_ascii_uppercase())
    } else {
        Err(InterProError::InvalidAccession(accession.to_owned()))
    }
}

fn api_error(status: reqwest::StatusCode, body: &str) -> InterProError {
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
    InterProError::Api {
        status: status.as_u16(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_interpro_accessions() {
        assert_eq!(normalize_accession(" ipr017861 ").unwrap(), "IPR017861");
        assert!(normalize_accession("IPR17861").is_err());
        assert!(normalize_accession("IPR01786A").is_err());
    }

    #[test]
    fn parses_entry_response() {
        let value = serde_json::json!({
            "metadata": {
                "accession": "IPR017861",
                "entry_type": "family",
                "source_database": "interpro",
                "name": {"name": "Kae1/TsaD family", "short": "KAE1/TsaD"},
                "description": [{"text": "Kae1/TsaD family"}],
                "counters": {"proteins": 26930}
            }
        });
        let response: EntryResponse = serde_json::from_value(value).unwrap();
        assert_eq!(response.metadata.accession, "IPR017861");
        assert_eq!(
            response.metadata.name.as_ref().unwrap().name,
            "Kae1/TsaD family"
        );
    }
}
