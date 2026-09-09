//! HTTP client and endpoint methods for the Reactome Content Service and
//! Analysis Service.

use serde::de::DeserializeOwned;

use crate::error::{ReactomeError, Result};
use crate::types::*;

/// Default Content Service endpoint.
pub const DEFAULT_CONTENT_ENDPOINT: &str = "https://reactome.org/ContentService";

/// Default Analysis Service endpoint.
pub const DEFAULT_ANALYSIS_ENDPOINT: &str = "https://reactome.org/AnalysisService";

const USER_AGENT: &str = "reactome-rs-sdk/0.1 (+https://reactome.org)";

fn content_endpoint() -> String {
    std::env::var("ENDPOINT_REACTOME_CS_URL")
        .unwrap_or_else(|_| DEFAULT_CONTENT_ENDPOINT.to_string())
}

fn analysis_endpoint() -> String {
    std::env::var("ENDPOINT_REACTOME_AS_URL")
        .unwrap_or_else(|_| DEFAULT_ANALYSIS_ENDPOINT.to_string())
}

fn require_id(id: &str) -> Result<&str> {
    let id = id.trim();
    if id.is_empty() {
        return Err(ReactomeError::InvalidIdentifier(id.to_string()));
    }
    Ok(id)
}

// ===========================================================================
// Client
// ===========================================================================

/// Async client for the Reactome Content Service and Analysis Service.
///
/// The API is free and open — no API key required. Both services sit behind
/// Cloudflare and accept anonymous requests; the client sets a polite
/// `User-Agent` and reuses one connection pool.
///
/// # Endpoints covered
///
/// | Method | API path |
/// |--------|----------|
/// | [`database_info`](Self::database_info) | Content `/data/database/{name,version}` |
/// | [`species_all`](Self::species_all) / [`species_main`](Self::species_main) | Content `/data/species/{all,main}` |
/// | [`top_level_pathways`](Self::top_level_pathways) | Content `/data/pathways/top/{species}` |
/// | [`events_hierarchy`](Self::events_hierarchy) | Content `/data/eventsHierarchy/{species}` |
/// | [`event_ancestors`](Self::event_ancestors) | Content `/data/event/{id}/ancestors` |
/// | [`query`](Self::query) | Content `/data/query/{id}` |
/// | [`map_to_pathways`](Self::map_to_pathways) | Content `/data/mapping/{resource}/{id}/pathways` |
/// | [`map_to_reactions`](Self::map_to_reactions) | Content `/data/mapping/{resource}/{id}/reactions` |
/// | [`participants`](Self::participants) | Content `/data/participants/{id}` |
/// | [`search`](Self::search) | Content `/search/query` |
/// | [`export_diagram`](Self::export_diagram) | Content `/exporter/diagram/{id}.{ext}` |
/// | [`analyse_identifiers`](Self::analyse_identifiers) | Analysis `POST /identifiers/projection` |
/// | [`analysis_result`](Self::analysis_result) | Analysis `GET /token/{token}` |
///
/// # Example
///
/// ```no_run
/// # use reactome::ReactomeClient;
/// # async fn run() -> reactome::Result<()> {
/// let client = ReactomeClient::new();
/// let info = client.database_info().await?;
/// println!("Reactome {} v{}", info.name, info.version);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct ReactomeClient {
    http: reqwest::Client,
    content_base: String,
    analysis_base: String,
}

impl Default for ReactomeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ReactomeClient {
    /// Create a production client, honoring `ENDPOINT_REACTOME_CS_URL` and
    /// `ENDPOINT_REACTOME_AS_URL`.
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("reqwest client builder");
        Self {
            http,
            content_base: content_endpoint(),
            analysis_base: analysis_endpoint(),
        }
    }

    /// Override both endpoints after construction.
    pub fn with_endpoints(
        mut self,
        content: impl Into<String>,
        analysis: impl Into<String>,
    ) -> Self {
        self.content_base = content.into();
        self.analysis_base = analysis.into();
        self
    }

    /// Borrow the inner reqwest client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// GET a Content Service path as typed JSON.
    ///
    /// `path` starts with `/`; query pairs are URL-encoded by reqwest.
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{}", self.content_base, path))
            .query(query)
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(ReactomeError::Status { status, body });
        }
        Ok(serde_json::from_str(&body)?)
    }

    // -----------------------------------------------------------------------
    // Database
    // -----------------------------------------------------------------------

    /// Database name and release version.
    pub async fn database_info(&self) -> Result<DatabaseInfo> {
        let name = self.get_text("/data/database/name").await?;
        let version = self.get_text("/data/database/version").await?;
        Ok(DatabaseInfo { name, version })
    }

    /// Raw Content Service text endpoint (escape hatch for `/data/...`).
    pub async fn get_text(&self, path: &str) -> Result<String> {
        let response = self
            .http
            .get(format!("{}{}", self.content_base, path))
            .header("Accept", "text/plain")
            .send()
            .await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(ReactomeError::Status { status, body });
        }
        Ok(body)
    }

    // -----------------------------------------------------------------------
    // Species
    // -----------------------------------------------------------------------

    /// All curated Reactome species.
    pub async fn species_all(&self) -> Result<Vec<Species>> {
        self.get("/data/species/all", &[]).await
    }

    /// Main species (human, mouse, rat, etc.).
    pub async fn species_main(&self) -> Result<Vec<Species>> {
        self.get("/data/species/main", &[]).await
    }

    // -----------------------------------------------------------------------
    // Pathways
    // -----------------------------------------------------------------------

    /// Top-level pathways for a species (e.g. `"Homo sapiens"`).
    pub async fn top_level_pathways(&self, species: &str) -> Result<Vec<Pathway>> {
        let species = require_id(species)?;
        let path = format!("/data/pathways/top/{}", urlencoding::encode(species));
        self.get(&path, &[]).await
    }

    /// Full event hierarchy tree for a species.
    pub async fn events_hierarchy(&self, species: &str) -> Result<Vec<EventAncestor>> {
        let species = require_id(species)?;
        let path = format!("/data/eventsHierarchy/{}", urlencoding::encode(species));
        self.get(&path, &[]).await
    }

    /// Ancestor chain from the root to a specific event.
    pub async fn event_ancestors(&self, id: &str) -> Result<Vec<EventAncestor>> {
        let id = require_id(id)?;
        self.get(&format!("/data/event/{id}/ancestors"), &[]).await
    }

    /// Low-level pathways containing a given physical entity.
    pub async fn entity_pathways(&self, id: &str) -> Result<Vec<Pathway>> {
        let id = require_id(id)?;
        self.get(&format!("/data/pathways/low/entity/{id}"), &[])
            .await
    }

    /// All events contained within a pathway.
    pub async fn contained_events(&self, id: &str) -> Result<Vec<Pathway>> {
        let id = require_id(id)?;
        self.get(&format!("/data/pathway/{id}/containedEvents"), &[])
            .await
    }

    // -----------------------------------------------------------------------
    // Query
    // -----------------------------------------------------------------------

    /// Query any Reactome object by stable ID or dbId.
    pub async fn query(&self, id: &str) -> Result<serde_json::Value> {
        let id = require_id(id)?;
        self.get(&format!("/data/query/{id}"), &[]).await
    }

    // -----------------------------------------------------------------------
    // Mapping
    // -----------------------------------------------------------------------

    /// Map an external identifier (e.g. UniProt accession) to Reactome pathways.
    ///
    /// Resource examples: `UniProt`, `Ensembl`, `EntrezGene`, `ChEBI`.
    pub async fn map_to_pathways(
        &self,
        resource: &str,
        identifier: &str,
    ) -> Result<Vec<MappedPathway>> {
        let resource = require_id(resource)?;
        let identifier = require_id(identifier)?;
        let path = format!(
            "/data/mapping/{}/{}/pathways",
            resource,
            urlencoding::encode(identifier)
        );
        self.get(&path, &[]).await
    }

    /// Map an external identifier to Reactome reactions.
    pub async fn map_to_reactions(
        &self,
        resource: &str,
        identifier: &str,
    ) -> Result<Vec<MappedPathway>> {
        let resource = require_id(resource)?;
        let identifier = require_id(identifier)?;
        let path = format!(
            "/data/mapping/{}/{}/reactions",
            resource,
            urlencoding::encode(identifier)
        );
        self.get(&path, &[]).await
    }

    // -----------------------------------------------------------------------
    // Participants
    // -----------------------------------------------------------------------

    /// All participants of a pathway or reaction.
    pub async fn participants(&self, id: &str) -> Result<Vec<Participant>> {
        let id = require_id(id)?;
        self.get(&format!("/data/participants/{id}"), &[]).await
    }

    /// Reference entities (cross-referenced IDs) of participants.
    pub async fn reference_entities(&self, id: &str) -> Result<Vec<Participant>> {
        let id = require_id(id)?;
        self.get(&format!("/data/participants/{id}/referenceEntities"), &[])
            .await
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Full-text search across Reactome content.
    pub async fn search(
        &self,
        query: &str,
        species: Option<&str>,
        types: Option<&[&str]>,
    ) -> Result<SearchResult> {
        let q = require_id(query)?;
        let mut query_pairs = vec![("query", q.to_string())];
        if let Some(s) = species {
            query_pairs.push(("species", s.to_string()));
        }
        if let Some(t) = types {
            query_pairs.push(("types", t.join(",")));
        }
        self.get("/search/query", &query_pairs).await
    }

    // -----------------------------------------------------------------------
    // Exporter
    // -----------------------------------------------------------------------

    /// Download a pathway diagram as raw bytes.
    ///
    /// `ext`: `"svg"`, `"png"`, `"jpg"`.
    pub async fn export_diagram(&self, stable_id: &str, ext: &str) -> Result<Vec<u8>> {
        let stable_id = require_id(stable_id)?;
        let ext = require_id(ext)?;
        let path = format!("/exporter/diagram/{stable_id}.{ext}");
        self.get_binary(&path).await
    }

    /// Download the whole-genome overview ("Fireworks") as raw bytes.
    pub async fn export_fireworks(&self, species: &str, ext: &str) -> Result<Vec<u8>> {
        let species = require_id(species)?;
        let ext = require_id(ext)?;
        let path = format!(
            "/exporter/fireworks/{}.{}",
            urlencoding::encode(species),
            ext
        );
        self.get_binary(&path).await
    }

    // -----------------------------------------------------------------------
    // Binary
    // -----------------------------------------------------------------------

    /// GET a Content Service path as raw bytes.
    pub async fn get_binary(&self, path: &str) -> Result<Vec<u8>> {
        let response = self
            .http
            .get(format!("{}{}", self.content_base, path))
            .send()
            .await?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let body = response.text().await.unwrap_or_default();
            return Err(ReactomeError::Status { status, body });
        }
        Ok(response.bytes().await?.to_vec())
    }

    // -----------------------------------------------------------------------
    // Analysis Service
    // -----------------------------------------------------------------------

    /// Submit a list of identifiers for over-representation analysis.
    ///
    /// When `project_to_human` is true, orthologs from other species are
    /// projected onto their human equivalents.
    pub async fn analyse_identifiers(
        &self,
        identifiers: &[String],
        project_to_human: bool,
    ) -> Result<AnalysisResult> {
        if identifiers.is_empty() {
            return Err(ReactomeError::InvalidParameter(
                "identifiers cannot be empty".to_string(),
            ));
        }
        let body = identifiers.join("\n");
        let suffix = if project_to_human { "/projection" } else { "" };
        let url = format!("{}{}", self.analysis_base, format!("/identifiers{suffix}"));
        let response = self
            .http
            .post(&url)
            .header("Content-Type", "text/plain")
            .header("Accept", "application/json")
            .body(body)
            .send()
            .await?;
        let status = response.status().as_u16();
        let text = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(ReactomeError::Status { status, body: text });
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Retrieve a previously computed analysis by token.
    pub async fn analysis_result(&self, token: &str) -> Result<AnalysisResult> {
        let token = require_id(token)?;
        self.get_analysis(&format!("/token/{token}")).await
    }

    /// Download enriched pathway results as CSV for a given token.
    pub async fn analysis_csv(&self, token: &str) -> Result<String> {
        let token = require_id(token)?;
        let url = format!(
            "{}/download/{token}/pathways/UniProt/pathways.csv",
            self.analysis_base
        );
        let response = self.http.get(&url).send().await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(ReactomeError::Status { status, body });
        }
        Ok(body)
    }

    /// GET an Analysis Service path as typed JSON.
    pub async fn get_analysis<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{}", self.analysis_base, path))
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(ReactomeError::Status { status, body });
        }
        Ok(serde_json::from_str(&body)?)
    }
}
