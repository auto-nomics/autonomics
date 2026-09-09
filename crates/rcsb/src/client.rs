use serde::de::DeserializeOwned;

use crate::error::{Result, api_error};
use crate::search::{SearchRequest, SearchResponse};
use crate::types::{Assembly, Entry, PolymerEntity};

pub const DATA_BASE: &str = "https://data.rcsb.org/rest/v1";
pub const SEARCH_BASE: &str = "https://search.rcsb.org/rcsbsearch/v2/query";
pub const FILES_BASE: &str = "https://files.rcsb.org/download";
pub const MODELS_BASE: &str = "https://models.rcsb.org";
pub const FASTA_BASE: &str = "https://www.rcsb.org/fasta/entry";

const USER_AGENT: &str = "rcsb-rs-sdk/0.1 (+https://www.rcsb.org)";
const MAX_SEARCH_ROWS: u32 = 10_000;

/// A bounded, decoded prefix of a text structure file.
#[derive(Debug, Clone)]
pub struct StructurePreview {
    pub entry_id: String,
    pub format: StructureFormat,
    pub url: String,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: usize,
    pub truncated: bool,
    pub text: String,
}

/// Structure file formats supported by the SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureFormat {
    Mmcif,
    Pdb,
    BinaryCif,
    Fasta,
}

impl StructureFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "cif" | "mmcif" | "mm cif" => Some(Self::Mmcif),
            "pdb" => Some(Self::Pdb),
            "bcif" | "binarycif" | "binary cif" => Some(Self::BinaryCif),
            "fasta" | "fa" => Some(Self::Fasta),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mmcif => "cif",
            Self::Pdb => "pdb",
            Self::BinaryCif => "bcif",
            Self::Fasta => "fasta",
        }
    }

    pub fn is_text(self) -> bool {
        !matches!(self, Self::BinaryCif)
    }
}

/// Async client for RCSB PDB Data, Search, and file endpoints.
///
/// All endpoints used by this client are public and require no API key.
#[derive(Debug, Clone)]
pub struct RcsbClient {
    http: reqwest::Client,
    data_base: String,
    search_url: String,
    files_base: String,
    models_base: String,
    fasta_base: String,
}

impl Default for RcsbClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RcsbClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("reqwest client builder");
        Self::from_parts(
            http,
            env("ENDPOINT_RCSB_DATA_URL", DATA_BASE),
            env("ENDPOINT_RCSB_SEARCH_URL", SEARCH_BASE),
            env("ENDPOINT_RCSB_FILES_URL", FILES_BASE),
            env("ENDPOINT_RCSB_MODELS_URL", MODELS_BASE),
            env("ENDPOINT_RCSB_FASTA_URL", FASTA_BASE),
        )
    }

    pub fn with_client(http: reqwest::Client) -> Self {
        Self::from_parts(
            http,
            env("ENDPOINT_RCSB_DATA_URL", DATA_BASE),
            env("ENDPOINT_RCSB_SEARCH_URL", SEARCH_BASE),
            env("ENDPOINT_RCSB_FILES_URL", FILES_BASE),
            env("ENDPOINT_RCSB_MODELS_URL", MODELS_BASE),
            env("ENDPOINT_RCSB_FASTA_URL", FASTA_BASE),
        )
    }
    fn from_parts(
        http: reqwest::Client,
        data_base: String,
        search_url: String,
        files_base: String,
        models_base: String,
        fasta_base: String,
    ) -> Self {
        Self {
            http,
            data_base,
            search_url,
            files_base,
            models_base,
            fasta_base,
        }
    }

    pub async fn entry(&self, entry_id: &str) -> Result<Entry> {
        self.get_json(&format!("/core/entry/{}", normalize_entry_id(entry_id)?))
            .await
    }

    pub async fn entries<I, S>(&self, entry_ids: I) -> Result<Vec<Entry>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let ids: Vec<String> = entry_ids
            .into_iter()
            .map(|id| normalize_entry_id(id.as_ref()))
            .collect::<Result<_>>()?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            entries.push(self.entry(&id).await?);
        }
        Ok(entries)
    }

    pub async fn polymer_entity(&self, entry_id: &str, entity_id: u32) -> Result<PolymerEntity> {
        self.get_json(&format!(
            "/core/polymer_entity/{}/{}",
            normalize_entry_id(entry_id)?,
            entity_id
        ))
        .await
    }

    pub async fn polymer_entities(
        &self,
        identifiers: &[(String, u32)],
    ) -> Result<Vec<PolymerEntity>> {
        if identifiers.is_empty() {
            return Ok(Vec::new());
        }

        let mut entities = Vec::with_capacity(identifiers.len());
        for (entry_id, entity_id) in identifiers {
            entities.push(self.polymer_entity(entry_id, *entity_id).await?);
        }
        Ok(entities)
    }

    pub async fn polymer_entities_for_entry(&self, entry_id: &str) -> Result<Vec<PolymerEntity>> {
        let normalized = normalize_entry_id(entry_id)?;
        let entry = self.entry(&normalized).await?;
        let identifiers = entry
            .rcsb_entry_container_identifiers
            .polymer_entity_ids
            .iter()
            .filter_map(|entity_id| entity_id.parse::<u32>().ok())
            .map(|entity_id| (normalized.clone(), entity_id))
            .collect::<Vec<_>>();
        self.polymer_entities(&identifiers).await
    }

    pub async fn assembly(&self, entry_id: &str, assembly_id: u32) -> Result<Assembly> {
        self.get_json(&format!(
            "/core/assembly/{}/{}",
            normalize_entry_id(entry_id)?,
            assembly_id
        ))
        .await
    }

    pub async fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        if request.request_options.paginate.rows == 0
            || request.request_options.paginate.rows > MAX_SEARCH_ROWS
        {
            return Err(crate::RcsbError::Param(format!(
                "search rows must be between 1 and {MAX_SEARCH_ROWS}"
            )));
        }

        let response = self
            .http
            .post(&self.search_url)
            .json(request)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    pub async fn search_raw(&self, query: serde_json::Value) -> Result<SearchResponse> {
        let response = self.http.post(&self.search_url).json(&query).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        serde_json::from_str(&body).map_err(Into::into)
    }

    pub fn structure_url(&self, entry_id: &str, format: StructureFormat) -> Result<String> {
        let id = normalize_entry_id(entry_id)?;
        Ok(match format {
            StructureFormat::Mmcif => format!("{}/{id}.cif", self.files_base),
            StructureFormat::Pdb => format!("{}/{id}.pdb", self.files_base),
            StructureFormat::BinaryCif => format!("{}/{id}.bcif", self.models_base),
            StructureFormat::Fasta => format!("{}/{id}", self.fasta_base),
        })
    }

    pub async fn structure_bytes(
        &self,
        entry_id: &str,
        format: StructureFormat,
    ) -> Result<Vec<u8>> {
        let url = self.structure_url(entry_id, format)?;
        let response = self.http.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(api_error(status, &body));
        }
        Ok(response.bytes().await?.to_vec())
    }

    pub async fn structure_text(&self, entry_id: &str, format: StructureFormat) -> Result<String> {
        if !format.is_text() {
            return Err(crate::RcsbError::BinaryText(format.as_str()));
        }
        let bytes = self.structure_bytes(entry_id, format).await?;
        String::from_utf8(bytes).map_err(|_| crate::RcsbError::BinaryText(format.as_str()))
    }

    /// Download at most `max_bytes` from a text structure endpoint.
    ///
    /// This is for previews. File-producing DAG nodes use
    /// [`Self::structure_bytes`] instead and retain the complete artifact.
    pub async fn structure_preview(
        &self,
        entry_id: &str,
        format: StructureFormat,
        max_bytes: usize,
    ) -> Result<StructurePreview> {
        if !format.is_text() {
            return Err(crate::RcsbError::Param(
                "structure previews support cif, pdb, and fasta text formats".into(),
            ));
        }
        let max_bytes = max_bytes.clamp(1024, 262_144);
        let url = self.structure_url(entry_id, format)?;
        let normalized = normalize_entry_id(entry_id)?;
        let mut response = self.http.get(&url).send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(api_error(status, &body));
        }

        let total_bytes = response.content_length();
        let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
        while bytes.len() < max_bytes {
            let Some(chunk) = response.chunk().await? else {
                break;
            };
            let remaining = max_bytes - bytes.len();
            if chunk.len() <= remaining {
                bytes.extend_from_slice(&chunk);
            } else {
                bytes.extend_from_slice(&chunk[..remaining]);
            }
        }
        let downloaded_bytes = bytes.len();
        let truncated = total_bytes
            .map(|total| downloaded_bytes < total as usize)
            .unwrap_or(downloaded_bytes == max_bytes);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Ok(StructurePreview {
            entry_id: normalized,
            format,
            url,
            total_bytes,
            downloaded_bytes,
            truncated,
            text,
        })
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{path}", self.data_base))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        serde_json::from_str(&body).map_err(Into::into)
    }
}

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

pub(crate) fn normalize_entry_id(entry_id: &str) -> Result<String> {
    let normalized = entry_id.trim().to_ascii_uppercase();
    if normalized.len() != 4 || !normalized.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(crate::RcsbError::InvalidEntryId(entry_id.to_owned()));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_ids_are_normalized_and_injection_rejected() {
        assert_eq!(normalize_entry_id("4hhb").unwrap(), "4HHB");
        assert!(normalize_entry_id("../secret").is_err());
        assert!(normalize_entry_id("123").is_err());
    }

    #[test]
    fn structure_formats_parse_and_classify() {
        assert_eq!(
            StructureFormat::parse("mmcif"),
            Some(StructureFormat::Mmcif)
        );
        assert_eq!(
            StructureFormat::parse(".bcif"),
            Some(StructureFormat::BinaryCif)
        );
        assert!(!StructureFormat::BinaryCif.is_text());
    }
}
