use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::Client;

use crate::error::{ArxivError, Result};
use crate::types::*;

/// Base URL for the arXiv API.
const BASE_URL: &str = "http://export.arxiv.org/api/query";

fn base_url() -> String {
    resource_catalog::endpoint_or("endpoint.arxiv", BASE_URL)
}

/// arXiv recommends at least 3 seconds between consecutive API calls.
const RATE_LIMIT_INTERVAL: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An async client for the [arXiv API](https://info.arxiv.org/help/api/index.html).
///
/// # Rate limits
///
/// arXiv requests a **3-second delay** between consecutive requests from the
/// same IP. `ArxivClient` enforces this automatically by sleeping before each
/// request when the previous request was less than 3 seconds ago.
///
/// # Paging
///
/// The API supports up to 2000 results per page and 30 000 total retrievable
/// results. Use [`SearchRequest::start`] and [`SearchRequest::max_results`]
/// for pagination.
///
/// # Example
///
/// ```no_run
/// use arxiv::ArxivClient;
///
/// let client = ArxivClient::new();
/// ```
#[derive(Debug)]
pub struct ArxivClient {
    client: Client,
    /// Timestamp of the last API request, used for rate-limiting.
    last_request: Mutex<Option<Instant>>,
}

impl Default for ArxivClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ArxivClient {
    /// Create a new client with default settings.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            last_request: Mutex::new(None),
        }
    }

    /// Compute how long to sleep (if at all) to respect the 3-second
    /// rate-limit interval, and record `now` as the last request time.
    fn rate_limit_sleep_duration(&self) -> Option<Duration> {
        let now = Instant::now();
        let mut lock = self.last_request.lock().unwrap();
        let prev = lock.get_or_insert(now - RATE_LIMIT_INTERVAL);
        let elapsed = now.duration_since(*prev);
        *lock = Some(now);
        if elapsed < RATE_LIMIT_INTERVAL {
            Some(RATE_LIMIT_INTERVAL - elapsed)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // Core request method
    // -----------------------------------------------------------------------

    /// Build the query URL and fetch the Atom XML response.
    ///
    /// Returns the raw XML text. The caller is responsible for parsing it.
    async fn get_atom(&self, params: &[(&str, &str)]) -> Result<String> {
        if let Some(d) = self.rate_limit_sleep_duration() {
            tokio::time::sleep(d).await;
        }

        let resp = self.client.get(&base_url()).query(params).send().await?;

        let status = resp.status().as_u16();
        let body = resp.text().await?;

        if !(200..300).contains(&status) {
            return Err(ArxivError::Status { status, body });
        }

        Ok(body)
    }

    // -----------------------------------------------------------------------
    // search — query by search expression
    // -----------------------------------------------------------------------

    /// Search arXiv for papers matching a query expression.
    ///
    /// Returns parsed [`SearchResponse`] with typed entries and pagination
    /// metadata.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use arxiv::{ArxivClient, types::SearchRequest};
    /// # async fn run() -> arxiv::error::Result<()> {
    /// let client = ArxivClient::new();
    /// let resp = client
    ///     .search(&SearchRequest::new("cat:cs.LG AND ti:transformer"))
    ///     .await?;
    /// println!("{} results", resp.total_results);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResponse> {
        let mut params: Vec<(&str, String)> = vec![("search_query", req.search_query.clone())];
        if let Some(n) = req.max_results {
            params.push(("max_results", n.to_string()));
        }
        if let Some(n) = req.start {
            params.push(("start", n.to_string()));
        }
        if let Some(ref sb) = req.sort_by {
            params.push(("sortBy", sb.as_str().to_owned()));
        }
        if let Some(ref so) = req.sort_order {
            params.push(("sortOrder", so.as_str().to_owned()));
        }

        let ref_params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let xml = self.get_atom(&ref_params).await?;

        crate::convert::parse_feed(&xml)
    }

    // -----------------------------------------------------------------------
    // fetch_by_id — retrieve specific papers by arXiv ID
    // -----------------------------------------------------------------------

    /// Fetch arXiv papers by their IDs.
    ///
    /// Accepts a comma-separated list of arXiv IDs (with or without version
    /// suffix). Old-style IDs (e.g. `"cond-mat/0207270"`) are also accepted.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use arxiv::{ArxivClient, types::FetchRequest};
    /// # async fn run() -> arxiv::error::Result<()> {
    /// let client = ArxivClient::new();
    /// let resp = client
    ///     .fetch_by_id(&FetchRequest::new("2401.12345,2309.01234v2"))
    ///     .await?;
    /// for entry in &resp.entries {
    ///     println!("{} — {}", entry.arxiv_id, entry.title);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_by_id(&self, req: &FetchRequest) -> Result<SearchResponse> {
        let mut params: Vec<(&str, String)> = vec![("id_list", req.id_list.clone())];
        if let Some(n) = req.max_results {
            params.push(("max_results", n.to_string()));
        }
        if let Some(n) = req.start {
            params.push(("start", n.to_string()));
        }

        let ref_params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let xml = self.get_atom(&ref_params).await?;

        let resp = crate::convert::parse_feed(&xml)?;

        // arXiv reports errors as entries with id starting with
        // "http://arxiv.org/api/errors#". Surface these as an error.
        if let Some(e) = resp.entries.first() {
            if e.arxiv_id.starts_with("api/errors") {
                return Err(ArxivError::Api(
                    e.summary.clone().unwrap_or_else(|| e.arxiv_id.clone()),
                ));
            }
        }

        Ok(resp)
    }
}
