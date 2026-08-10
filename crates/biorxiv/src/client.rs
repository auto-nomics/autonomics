use reqwest::Client;

use crate::error::{BiorxivError, Result};
use crate::types::*;

/// Base URL for the bioRxiv/medRxiv API.
///
/// Both `api.medrxiv.org` and `api.biorxiv.org` serve identical APIs; the
/// `{server}` path segment selects the content pool. We use `api.biorxiv.org`
/// as the canonical base since it additionally hosts the `/pub` endpoint.
const BASE_URL: &str = "https://api.biorxiv.org";

fn base_url() -> String {
    resource_catalog::endpoint_or("endpoint.biorxiv", BASE_URL)
}

/// Maximum results returned per API call.
const PAGE_SIZE: u32 = 100;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An async client for the [bioRxiv/medRxiv API](https://api.biorxiv.org/).
///
/// Both servers share a single API surface; select content via the
/// [`Server`] parameter. The API has **no keyword search** — it supports
/// browsing by date range, recency, or fetching a specific DOI.
///
/// # Pagination
///
/// Each call returns at most **100 papers**. Use the `cursor` parameter
/// (0-based) to page through larger result sets. The [`details_all`]
/// method auto-paginates.
///
/// # Example
///
/// ```no_run
/// use biorxiv::{BiorxivClient, types::{Server, Interval}};
/// use chrono::NaiveDate;
///
/// # async fn run() -> biorxiv::error::Result<()> {
/// let client = BiorxivClient::new();
/// let resp = client
///     .details_by_date(
///         Server::Medrxiv,
///         NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
///         NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
///         0,
///     )
///     .await?;
/// println!("{} papers", resp.collection.len());
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct BiorxivClient {
    client: Client,
}

impl Default for BiorxivClient {
    fn default() -> Self {
        Self::new()
    }
}

impl BiorxivClient {
    /// Create a new client with default settings.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Core request
    // -----------------------------------------------------------------------

    /// Execute a GET request and deserialise the JSON envelope.
    async fn get<T>(&self, url: &str) -> Result<ApiResponse<T>>
    where
        T: serde::de::DeserializeOwned + serde::Serialize,
    {
        let resp = self.client.get(url).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;

        if !(200..300).contains(&status) {
            return Err(BiorxivError::Status { status, body });
        }

        let parsed: ApiResponse<T> = serde_json::from_str(&body)?;

        // Surface API-level errors from the messages array.
        if let Some(msg) = parsed.messages.first() {
            if !msg.is_ok() {
                return Err(BiorxivError::Api(msg.status.clone()));
            }
        }

        Ok(parsed)
    }

    // -----------------------------------------------------------------------
    // Details — by DOI
    // -----------------------------------------------------------------------

    /// Fetch all versions of a single manuscript by its DOI.
    ///
    /// Returns every posted version of the paper (v1, v2, …) in the
    /// `collection` array. Use [`crate::convert::latest_version`] to pick
    /// the newest.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use biorxiv::{BiorxivClient, types::Server};
    /// # async fn run() -> biorxiv::error::Result<()> {
    /// let client = BiorxivClient::new();
    /// let resp = client
    ///     .details_by_doi(Server::Medrxiv, "10.1101/2023.10.21.23297352")
    ///     .await?;
    /// println!("{} versions", resp.collection.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn details_by_doi(&self, server: Server, doi: &str) -> Result<DetailsResponse> {
        let url = format!(
            "{base}/details/{server}/{doi}/na/json",
            base = base_url(),
            server = server.segment(),
            doi = doi.trim(),
        );
        self.get(&url).await
    }

    // -----------------------------------------------------------------------
    // Details — by interval
    // -----------------------------------------------------------------------

    /// Fetch a single page of papers matching the given interval.
    ///
    /// Each call returns at most 100 papers; increment `cursor` to page.
    pub async fn details_by_interval(
        &self,
        server: Server,
        interval: &Interval,
        cursor: u32,
    ) -> Result<DetailsResponse> {
        // The API URL structure differs by interval type:
        // - Recent(N) / Days(N): `details/{server}/{N}` — no cursor, no format
        // - DateRange: `details/{server}/{from}/{to}/{cursor}` — with cursor
        let url = match interval {
            Interval::Recent(n) => {
                format!(
                    "{base}/details/{server}/{n}",
                    base = base_url(),
                    server = server.segment(),
                )
            }
            Interval::Days(n) => {
                format!(
                    "{base}/details/{server}/{n}d",
                    base = base_url(),
                    server = server.segment(),
                )
            }
            Interval::DateRange { from, to } => {
                format!(
                    "{base}/details/{server}/{from}/{to}/{cursor}",
                    base = base_url(),
                    server = server.segment(),
                )
            }
        };
        self.get(&url).await
    }

    /// Fetch the *N* most recent papers from the given server.
    pub async fn details_recent(&self, server: Server, n: u32) -> Result<DetailsResponse> {
        self.details_by_interval(server, &Interval::Recent(n), 0)
            .await
    }

    /// Fetch papers posted within the last *N* days.
    ///
    /// Paginated by `cursor` (0-based, 100 per page).
    pub async fn details_by_days(
        &self,
        server: Server,
        days: u32,
        cursor: u32,
    ) -> Result<DetailsResponse> {
        self.details_by_interval(server, &Interval::Days(days), cursor)
            .await
    }

    /// Fetch papers posted within an inclusive date range.
    ///
    /// Paginated by `cursor` (0-based, 100 per page).
    pub async fn details_by_date(
        &self,
        server: Server,
        from: chrono::NaiveDate,
        to: chrono::NaiveDate,
        cursor: u32,
    ) -> Result<DetailsResponse> {
        self.details_by_interval(server, &Interval::DateRange { from, to }, cursor)
            .await
    }

    /// Fetch **all** papers in an interval, auto-paginating until exhausted
    /// or `max_results` is reached.
    ///
    /// This is a convenience wrapper around [`details_by_interval`] that
    /// follows the cursor automatically.
    pub async fn details_all(
        &self,
        server: Server,
        interval: &Interval,
        max_results: usize,
    ) -> Result<DetailsResponse> {
        // Recent(N) and Days(N) return all results in a single call —
        // no pagination is supported by the API.
        if matches!(interval, Interval::Recent(_) | Interval::Days(_)) {
            let mut resp = self.details_by_interval(server, interval, 0).await?;
            if resp.collection.len() > max_results {
                resp.collection.truncate(max_results);
            }
            return Ok(resp);
        }

        // DateRange: auto-paginate.
        let mut all = Vec::new();
        let mut cursor = 0u32;
        let mut total = None;

        loop {
            let resp = self.details_by_interval(server, interval, cursor).await?;

            // Capture total from the first page.
            if total.is_none() {
                if let Some(msg) = resp.messages.first() {
                    total = msg.total.or(msg.count);
                }
            }

            let page_len = resp.collection.len();
            all.extend(resp.collection);

            if all.len() >= max_results {
                all.truncate(max_results);
                break;
            }

            // Stop if we got fewer than a full page (exhausted) or nothing.
            if page_len < PAGE_SIZE as usize || page_len == 0 {
                break;
            }

            // Stop if we've already fetched everything.
            if let Some(t) = total {
                if all.len() >= t as usize {
                    break;
                }
            }

            cursor += 1;
        }

        Ok(DetailsResponse {
            messages: vec![Message {
                status: "ok".into(),
                total: Some(all.len() as u64),
                count: Some(all.len() as u64),
                ..Default::default()
            }],
            collection: all,
        })
    }

    // -----------------------------------------------------------------------
    // Pub endpoint — preprint → published mappings
    // -----------------------------------------------------------------------

    /// Fetch preprint-to-published-article mappings for a date range.
    ///
    /// Only available on `api.biorxiv.org`. Paginated by `cursor`.
    pub async fn pub_by_date(
        &self,
        from: chrono::NaiveDate,
        to: chrono::NaiveDate,
        cursor: u32,
    ) -> Result<PubResponse> {
        let url = format!("{base}/pub/{from}/{to}/{cursor}", base = base_url(),);
        self.get(&url).await
    }
}
