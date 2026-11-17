//! BioIcons catalogue adapter. Licences belong to individual icons, not the site.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::OnceCell;
use url::Url;
pub mod geometry;
pub mod parts;
pub mod vector;
mod vector_cache;
pub const DEFAULT_ENDPOINT: &str = "https://bioicons.com/";
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP transport: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid BioIcons data: {0}")]
    Invalid(String),
    #[error("asset not found")]
    NotFound,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub category: String,
    pub license: String,
    pub license_url: Option<String>,
    pub author: String,
    pub author_url: Option<String>,
    pub source_url: String,
    pub svg_url: String,
    pub catalog_sha256: String,
}
#[derive(Debug, Serialize)]
pub struct SearchPage {
    pub total: usize,
    pub start: usize,
    pub assets: Vec<Asset>,
    pub match_summary: SearchMatchSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_filter: Option<String>,
}
/// Counts cover all lexical matches before pagination, not biological identity.
#[derive(Debug, Default, Serialize)]
pub struct SearchMatchSummary {
    pub exact_name: usize,
    pub name_tokens: usize,
    pub name_prefix: usize,
    pub name_substring: usize,
    pub metadata_tokens: usize,
    pub metadata_substring: usize,
}
// Lexical relevance only. Exact names and complete name tokens precede
// prefixes/substrings; matching an icon does not establish scientific identity.
fn search_rank(asset: &Asset, terms: &[String]) -> u8 {
    let name = asset.name.to_lowercase().replace('_', " ");
    let words = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let tokens = words(&name);
    if name.trim() == terms.join(" ") {
        return 0;
    }
    if terms.iter().all(|term| tokens.contains(term)) {
        return 1;
    }
    if terms
        .iter()
        .all(|term| tokens.iter().any(|word| word.starts_with(term)))
    {
        return 2;
    }
    if terms.iter().all(|term| name.contains(term)) {
        return 3;
    }
    let hay = format!("{} {} {}", name, asset.category, asset.author).to_lowercase();
    let tokens = words(&hay);
    if terms.iter().all(|term| tokens.contains(term)) {
        4
    } else {
        5
    }
}

#[derive(Deserialize)]
struct Row {
    name: String,
    category: String,
    license: String,
    author: String,
}
#[derive(Clone)]
pub struct BioIconsClient {
    http: reqwest::Client,
    endpoint: Url,
    catalog: Arc<OnceCell<Vec<Asset>>>,
    vectors: Arc<tokio::sync::Mutex<vector_cache::VectorCache>>,
}
impl BioIconsClient {
    pub fn new() -> Result<Self> {
        Self::with_endpoint(DEFAULT_ENDPOINT)
    }
    pub fn with_endpoint(endpoint: &str) -> Result<Self> {
        let endpoint = Url::parse(endpoint).map_err(|e| Error::Invalid(e.to_string()))?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || endpoint.path() != "/"
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(Error::Invalid("endpoint must be an HTTP(S) origin".into()));
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(45))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("autonomics-bioicons/0.1")
                .build()?,
            endpoint,
            catalog: Arc::new(OnceCell::new()),
            vectors: Arc::new(tokio::sync::Mutex::new(vector_cache::VectorCache::default())),
        })
    }
    async fn bytes(&self, url: Url, limit: usize) -> Result<Vec<u8>> {
        let mut response = self.http.get(url).send().await?.error_for_status()?;
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(Error::Invalid("upstream response too large".into()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > limit {
                return Err(Error::Invalid("upstream response too large".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn catalogue(&self) -> Result<&Vec<Asset>> {
        self.catalog
            .get_or_try_init(|| async {
                let bytes = self
                    .bytes(
                        self.endpoint.join("icons/icons.json").unwrap(),
                        4 * 1024 * 1024,
                    )
                    .await?;
                let rows: Vec<Row> =
                    serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(e.to_string()))?;
                if rows.len() > 20000 {
                    return Err(Error::Invalid("catalogue exceeds 20000 entries".into()));
                }
                let authors: BTreeMap<String, String> = serde_json::from_slice(
                    &self
                        .bytes(
                            self.endpoint.join("icons/authors.json").unwrap(),
                            1024 * 1024,
                        )
                        .await?,
                )
                .map_err(|e| Error::Invalid(e.to_string()))?;
                let hash = format!("{:x}", Sha256::digest(&bytes));
                rows.into_iter()
                    .map(|row| self.asset(row, &authors, &hash))
                    .collect()
            })
            .await
    }
    fn asset(&self, row: Row, authors: &BTreeMap<String, String>, hash: &str) -> Result<Asset> {
        for value in [&row.name, &row.category, &row.license, &row.author] {
            if value.trim().is_empty()
                || value.len() > 300
                || value == "."
                || value == ".."
                || value
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            {
                return Err(Error::Invalid("invalid catalogue path component".into()));
            }
        }
        let mut url = self.endpoint.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| Error::Invalid("invalid origin".into()))?;
            segments.pop_if_empty().extend([
                "icons",
                &row.license,
                &row.category,
                &row.author.replacen(' ', "_", 1),
                &format!("{}.svg", row.name),
            ]);
        }
        let id = format!("{:x}", Sha256::digest(url.path().as_bytes()));
        let mut source = self.endpoint.clone();
        source.query_pairs_mut().append_pair("query", &row.name);
        let author_url = authors
            .get(&row.author)
            .filter(|u| Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https")))
            .cloned();
        let license_url = match row.license.as_str() {
            "cc-0" => Some("https://creativecommons.org/publicdomain/zero/1.0/"),
            "cc-by-3.0" => Some("https://creativecommons.org/licenses/by/3.0/"),
            "cc-by-4.0" => Some("https://creativecommons.org/licenses/by/4.0/"),
            "cc-by-sa-3.0" => Some("https://creativecommons.org/licenses/by-sa/3.0/"),
            "cc-by-sa-4.0" => Some("https://creativecommons.org/licenses/by-sa/4.0/"),
            "mit" => Some("https://mit-license.org/"),
            "bsd" => Some("https://opensource.org/licenses/BSD-3-Clause"),
            _ => None,
        }
        .map(str::to_owned);
        Ok(Asset {
            id,
            name: row.name,
            category: row.category,
            license: row.license,
            license_url,
            author: row.author,
            author_url,
            source_url: source.into(),
            svg_url: url.into(),
            catalog_sha256: hash.into(),
        })
    }
    pub async fn search(&self, query: &str, start: usize, limit: usize) -> Result<SearchPage> {
        self.search_with_author(query, start, limit, None).await
    }
    /// Exact author-name filter, ignoring case and surrounding whitespace.
    /// This restricts attribution, not scientific identity or visual style.
    pub async fn search_with_author(
        &self,
        query: &str,
        start: usize,
        limit: usize,
        author: Option<&str>,
    ) -> Result<SearchPage> {
        if author.is_some_and(|a| a.trim().is_empty() || a.len() > 300) {
            return Err(Error::Invalid(
                "author must be a nonblank name of at most 300 UTF-8 bytes".into(),
            ));
        }
        let author = author.map(str::trim);
        if query.trim().is_empty() || query.len() > 300 || limit == 0 || limit > 100 {
            return Err(Error::Invalid(
                "requires a query of 1..300 bytes and limit 1..100".into(),
            ));
        }
        let terms = query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut matches = self
            .catalogue()
            .await?
            .iter()
            .filter(|a| author.is_none_or(|name| a.author.to_lowercase() == name.to_lowercase()))
            .filter(|a| {
                let hay = format!("{} {} {}", a.name, a.category, a.author)
                    .to_lowercase()
                    .replace('_', " ");
                terms.iter().all(|t| hay.contains(t))
            })
            .collect::<Vec<_>>();
        // Stable sort preserves catalogue order for equal relevance and keeps
        // pagination deterministic for a fixed catalogue snapshot.
        matches.sort_by_key(|asset| search_rank(asset, &terms));
        let mut match_summary = SearchMatchSummary::default();
        for asset in &matches {
            let count = match search_rank(asset, &terms) {
                0 => &mut match_summary.exact_name,
                1 => &mut match_summary.name_tokens,
                2 => &mut match_summary.name_prefix,
                3 => &mut match_summary.name_substring,
                4 => &mut match_summary.metadata_tokens,
                _ => &mut match_summary.metadata_substring,
            };
            *count += 1;
        }
        Ok(SearchPage {
            author_filter: author.map(str::to_owned),
            match_summary,
            total: matches.len(),
            start,
            assets: matches
                .into_iter()
                .skip(start)
                .take(limit)
                .cloned()
                .collect(),
        })
    }
    pub async fn vector(&self, id: &str) -> Result<vector::VectorAsset> {
        if let Some(asset) = self.vectors.lock().await.get(id) {
            return Ok(asset);
        }
        let asset = self
            .catalogue()
            .await?
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or(Error::NotFound)?;
        let bytes = self
            .bytes(
                Url::parse(&asset.svg_url).map_err(|e| Error::Invalid(e.to_string()))?,
                4 * 1024 * 1024,
            )
            .await?;
        let prepared = tokio::task::spawn_blocking(move || vector::prepare(asset, &bytes))
            .await
            .map_err(|e| Error::Invalid(e.to_string()))??;
        self.vectors
            .lock()
            .await
            .insert(id.to_owned(), prepared.clone());
        Ok(prepared)
    }
}
