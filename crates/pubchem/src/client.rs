use serde::de::DeserializeOwned;
use url::Url;

use crate::error::{PubChemError, Result};
use crate::types::{CompoundIdentifierType, CompoundProperties, PropertyResponse};

pub const DEFAULT_ENDPOINT: &str = "https://pubchem.ncbi.nlm.nih.gov/rest/pug";
pub const REQUESTED_PROPERTIES: &str =
    "MolecularFormula,MolecularWeight,SMILES,ConnectivitySMILES,InChI,InChIKey,IUPACName,Title";

#[derive(Debug, Clone)]
pub struct PubChemClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for PubChemClient {
    fn default() -> Self {
        Self::new()
    }
}

impl PubChemClient {
    pub fn new() -> Self {
        let endpoint =
            std::env::var("ENDPOINT_PUBCHEM_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        Self::with_endpoint(endpoint)
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("pubchem-rs-sdk/0.1 (+https://pubchem.ncbi.nlm.nih.gov)")
                .build()
                .expect("failed to build HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn compound(
        &self,
        identifier_type: CompoundIdentifierType,
        identifier: &str,
    ) -> Result<Option<CompoundProperties>> {
        let identifier = identifier.trim();
        validate_identifier(identifier_type, identifier)?;
        let mut url = Url::parse(&self.endpoint)?;
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                PubChemError::InvalidIdentifier("endpoint cannot contain base segments".into())
            })?;
            segments
                .pop_if_empty()
                .push("compound")
                .push(identifier_type.as_str())
                .push(identifier)
                .push("property")
                .push(REQUESTED_PROPERTIES)
                .push("JSON");
        }

        let response = self.http.get(url).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if status.as_u16() == 404 {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        let response: PropertyResponse = serde_json::from_str(&body)?;
        Ok(response.property_table.properties.into_iter().next())
    }
}

fn validate_identifier(identifier_type: CompoundIdentifierType, identifier: &str) -> Result<()> {
    if identifier.is_empty() {
        return Err(PubChemError::InvalidIdentifier(identifier.to_owned()));
    }
    match identifier_type {
        CompoundIdentifierType::Name => {
            if identifier
                .chars()
                .all(|ch| !matches!(ch, '/' | '\\' | '?' | '#' | '%'))
            {
                Ok(())
            } else {
                Err(PubChemError::InvalidIdentifier(identifier.to_owned()))
            }
        }
        CompoundIdentifierType::Cid => identifier
            .parse::<u64>()
            .map(|_| ())
            .map_err(|_| PubChemError::InvalidIdentifier(identifier.to_owned())),
        CompoundIdentifierType::InChIKey => {
            if identifier.len() == 27
                && identifier
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            {
                Ok(())
            } else {
                Err(PubChemError::InvalidIdentifier(identifier.to_owned()))
            }
        }
    }
}

fn api_error(status: reqwest::StatusCode, body: &str) -> PubChemError {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/Fault/Details/Message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    value
                        .pointer("/Fault/Summary")
                        .and_then(serde_json::Value::as_str)
                })
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
    PubChemError::Api {
        status: status.as_u16(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_property_response() {
        let value = serde_json::json!({
            "PropertyTable": {
                "Properties": [{
                    "CID": 2244,
                    "MolecularFormula": "C9H8O4",
                    "MolecularWeight": "180.16",
                    "SMILES": "CC(=O)OC1=CC=CC=C1C(=O)O",
                    "Title": "Aspirin"
                }]
            }
        });
        let response: PropertyResponse = serde_json::from_value(value).unwrap();
        assert_eq!(response.property_table.properties[0].cid, Some(2244));
        assert_eq!(
            response.property_table.properties[0]
                .molecular_formula
                .as_deref(),
            Some("C9H8O4")
        );
    }

    #[test]
    fn validates_identifiers() {
        assert!(validate_identifier(CompoundIdentifierType::Cid, "2244").is_ok());
        assert!(validate_identifier(CompoundIdentifierType::Cid, "aspirin").is_err());
        assert!(validate_identifier(CompoundIdentifierType::Name, "acetyl salicylic acid").is_ok());
        assert!(validate_identifier(CompoundIdentifierType::Name, "bad/name").is_err());
    }
}
