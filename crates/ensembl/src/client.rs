//! HTTP client and endpoint methods for Ensembl REST.

use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;

use crate::error::{EnsemblError, Result};
use crate::sequence::{SequenceType, sequence_params};
use crate::types::*;

/// Default production Ensembl REST endpoint.
pub const DEFAULT_ENSEMBL_ENDPOINT: &str = "https://rest.ensembl.org";
const USER_AGENT: &str = "ensembl-rs-sdk/0.1 (+https://rest.ensembl.org)";

fn endpoint() -> String {
    std::env::var("ENDPOINT_ENSEMBL_URL").unwrap_or_else(|_| DEFAULT_ENSEMBL_ENDPOINT.to_string())
}

fn require_id(id: &str) -> Result<&str> {
    let id = id.trim();
    if id.is_empty() {
        return Err(EnsemblError::InvalidIdentifier(id.to_string()));
    }
    Ok(id)
}

fn require_species(species: &str) -> Result<&str> {
    let species = species.trim();
    if species.is_empty() {
        return Err(EnsemblError::InvalidParameter(
            "species cannot be empty".to_string(),
        ));
    }
    Ok(species)
}

fn require_region(region: &str) -> Result<&str> {
    let region = region.trim();
    if region.is_empty() {
        return Err(EnsemblError::InvalidRegion(region.to_string()));
    }
    Ok(region)
}

/// Async client for the Ensembl REST API.
///
/// All supported typed helpers share one reqwest client and endpoint. The
/// generic [`get`](Self::get) method is an escape hatch for endpoints that
/// have not yet received typed models.
#[derive(Debug, Clone)]
pub struct EnsemblClient {
    http: reqwest::Client,
    base_url: String,
}

impl Default for EnsemblClient {
    fn default() -> Self {
        Self::new()
    }
}

impl EnsemblClient {
    /// Create a production client, honoring `ENDPOINT_ENSEMBL_URL`.
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("reqwest client builder");
        Self {
            http,
            base_url: endpoint(),
        }
    }

    /// Use a custom HTTP client and base endpoint.
    pub fn with_client_and_endpoint(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into(),
        }
    }

    /// Override only the endpoint after construction.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.base_url = endpoint.into();
        self
    }

    /// Borrow the inner reqwest client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Get the configured base endpoint.
    pub fn endpoint(&self) -> &str {
        &self.base_url
    }

    /// GET an arbitrary Ensembl REST path as typed JSON.
    ///
    /// `path` starts with `/`; query pairs are URL-encoded by reqwest.
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .query(query)
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(EnsemblError::Status { status, body });
        }
        Ok(serde_json::from_str(&body)?)
    }

    /// POST arbitrary JSON to an Ensembl REST path.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let response = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("Accept", "application/json")
            .json(body)
            .send()
            .await?;
        let status = response.status().as_u16();
        let body_text = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(EnsemblError::Status {
                status,
                body: body_text,
            });
        }
        Ok(serde_json::from_str(&body_text)?)
    }

    /// GET `/info/species`.
    pub async fn species(&self) -> Result<SpeciesResponse> {
        self.get("/info/species", &[]).await
    }

    /// GET `/info/assembly/{species}`.
    pub async fn assembly(&self, species: &str) -> Result<AssemblyInfo> {
        let species = require_species(species)?;
        self.get(&format!("/info/assembly/{species}"), &[]).await
    }

    /// GET `/lookup/id/{id}`, optionally expanding child transcript/exons.
    pub async fn lookup_id(&self, id: &str, expand: bool) -> Result<LookupEntry> {
        let id = require_id(id)?;
        let query = vec![("expand", expand.to_string())];
        self.get(&format!("/lookup/id/{id}"), &query).await
    }

    /// GET `/lookup/id/{species}/{id}`.
    pub async fn lookup_id_with_species(
        &self,
        species: &str,
        id: &str,
        expand: bool,
    ) -> Result<LookupEntry> {
        let species = require_species(species)?;
        let id = require_id(id)?;
        let query = vec![("expand", expand.to_string())];
        self.get(&format!("/lookup/id/{species}/{id}"), &query)
            .await
    }

    /// POST `/lookup/id` for up to 1000 identifiers.
    pub async fn lookup_ids(&self, ids: &[String], expand: bool) -> Result<HashMap<String, Value>> {
        if ids.is_empty() {
            return Err(EnsemblError::EmptyBatch);
        }
        if ids.len() > 1000 {
            return Err(EnsemblError::InvalidParameter(
                "lookup batches support at most 1000 identifiers".to_string(),
            ));
        }
        let body = serde_json::json!({ "ids": ids, "expand": expand });
        self.post("/lookup/id", &body).await
    }

    /// GET `/xrefs/id/{id}`.
    pub async fn xrefs(
        &self,
        id: &str,
        all_levels: bool,
        external_db: Option<&str>,
    ) -> Result<Vec<Xref>> {
        let id = require_id(id)?;
        let mut query = vec![("all_levels", all_levels.to_string())];
        if let Some(db) = external_db {
            query.push(("external_db", db.to_string()));
        }
        self.get(&format!("/xrefs/id/{id}"), &query).await
    }

    /// GET `/overlap/region/{species}/{region}`.
    pub async fn overlap_region(
        &self,
        species: &str,
        region: &str,
        features: &[String],
    ) -> Result<Vec<Value>> {
        let species = require_species(species)?;
        let region = require_region(region)?;
        if features.is_empty() {
            return Err(EnsemblError::InvalidParameter(
                "at least one overlap feature type is required".to_string(),
            ));
        }
        let query: Vec<(&str, String)> = features.iter().map(|f| ("feature", f.clone())).collect();
        self.get(&format!("/overlap/region/{species}/{region}"), &query)
            .await
    }

    /// GET `/overlap/id/{id}`.
    pub async fn overlap_id(&self, id: &str, features: &[String]) -> Result<Vec<Value>> {
        let id = require_id(id)?;
        if features.is_empty() {
            return Err(EnsemblError::InvalidParameter(
                "at least one overlap feature type is required".to_string(),
            ));
        }
        let query: Vec<(&str, String)> = features.iter().map(|f| ("feature", f.clone())).collect();
        self.get(&format!("/overlap/id/{id}"), &query).await
    }

    /// GET `/sequence/id/{id}`.
    pub async fn sequence_id(
        &self,
        id: &str,
        sequence_type: SequenceType,
        expand_5prime: Option<u64>,
        expand_3prime: Option<u64>,
    ) -> Result<Sequence> {
        let id = require_id(id)?;
        let query = sequence_params(sequence_type, expand_5prime, expand_3prime);
        self.get(&format!("/sequence/id/{id}"), &query).await
    }

    /// GET `/sequence/region/{species}/{region}`.
    pub async fn sequence_region(
        &self,
        species: &str,
        region: &str,
        sequence_type: SequenceType,
        expand_5prime: Option<u64>,
        expand_3prime: Option<u64>,
    ) -> Result<Sequence> {
        let species = require_species(species)?;
        let region = require_region(region)?;
        let query = sequence_params(sequence_type, expand_5prime, expand_3prime);
        self.get(&format!("/sequence/region/{species}/{region}"), &query)
            .await
    }

    /// GET `/vep/{species}/id/{id}`.
    pub async fn vep_id(&self, species: &str, id: &str) -> Result<Vec<VepResult>> {
        let species = require_species(species)?;
        let id = require_id(id)?;
        self.get(&format!("/vep/{species}/id/{id}"), &[]).await
    }

    /// GET `/vep/{species}/region/{region}`.
    pub async fn vep_region(
        &self,
        species: &str,
        region: &str,
        allele: &str,
    ) -> Result<Vec<VepResult>> {
        let species = require_species(species)?;
        let region = require_region(region)?;
        let allele = allele.trim();
        if allele.is_empty() {
            return Err(EnsemblError::InvalidParameter(
                "VEP region requests require an allele".to_string(),
            ));
        }
        self.get(&format!("/vep/{species}/region/{region}/{allele}"), &[])
            .await
    }

    /// GET `/variation/{species}/{id}`.
    pub async fn variation(&self, species: &str, id: &str) -> Result<Variation> {
        let species = require_species(species)?;
        let id = require_id(id)?;
        self.get(&format!("/variation/{species}/{id}"), &[]).await
    }
}
