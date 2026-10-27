use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use reqwest::Client;

use crate::error::{ArxivError, Result};
use crate::types::*;

fn env_endpoint(env: &str, fallback: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| fallback.to_string())
}

/// Base URL for the arXiv API.
const BASE_URL: &str = "https://export.arxiv.org/api/query";

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
/// Requests use a 10-second connection timeout and 90-second HTTP timeout;
/// callers can impose a shorter budget including time spent waiting to send.
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
    base_url: String,
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
            client: Client::builder()
                .user_agent(concat!("autonomics-arxiv/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(90))
                .build()
                .expect("build arXiv HTTP client"),
            last_request: Mutex::new(None),
            base_url: env_endpoint("ENDPOINT_ARXIV_URL", BASE_URL),
        }
    }

    /// Override this client's endpoint without mutating process environment.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    // -----------------------------------------------------------------------
    // Core request method
    // -----------------------------------------------------------------------

    /// Build the query URL and fetch the Atom XML response.
    ///
    /// Returns the raw XML text. The caller is responsible for parsing it.
    async fn get_atom(&self, params: &[(&str, &str)]) -> Result<String> {
        // Hold the same async lock through the body read: arXiv permits one
        // connection at a time, including calls made outside discovery.
        let mut last_request = self.last_request.lock().await;
        if let Some(previous) = *last_request {
            tokio::time::sleep(RATE_LIMIT_INTERVAL.saturating_sub(previous.elapsed())).await;
        }
        *last_request = Some(Instant::now());
        let request = self.client.get(&self.base_url).query(params).build()?;
        let mut resp = if request.url().as_str().len() > 4094 {
            self.client.post(&self.base_url).form(params).send().await?
        } else {
            self.client.execute(request).await?
        };
        let status = resp.status().as_u16();
        if status == 429 {
            let retry_after_seconds = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.trim().parse::<u64>().ok().or_else(|| {
                        httpdate::parse_http_date(v.trim()).ok().map(|at| {
                            let delay = at
                                .duration_since(std::time::SystemTime::now())
                                .unwrap_or_default();
                            delay
                                .as_secs()
                                .saturating_add(u64::from(delay.subsec_nanos() > 0))
                        })
                    })
                });
            return Err(ArxivError::RateLimited {
                retry_after_seconds,
            });
        }
        let limit = if (200..300).contains(&status) {
            16 * 1024 * 1024
        } else {
            64 * 1024
        };
        if resp
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(if (200..300).contains(&status) {
                ArxivError::ResponseTooLarge
            } else {
                ArxivError::Status {
                    status,
                    body: "response body omitted (size limit)".into(),
                }
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(if (200..300).contains(&status) {
                    ArxivError::ResponseTooLarge
                } else {
                    ArxivError::Status {
                        status,
                        body: "response body omitted (size limit)".into(),
                    }
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        if !(200..300).contains(&status) {
            return Err(ArxivError::Status {
                status,
                body: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        let body = String::from_utf8(bytes)
            .map_err(|_| ArxivError::Xml("response is not UTF-8".into()))?;
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
        validate_window(req.start, req.max_results)?;
        if req.search_query.trim().is_empty()
            && req
                .id_list
                .as_deref()
                .is_none_or(|ids| ids.trim().is_empty())
        {
            return Err(ArxivError::Param("provide search_query or id_list".into()));
        }
        let mut params: Vec<(&str, String)> = Vec::new();
        if !req.search_query.is_empty() {
            params.push(("search_query", req.search_query.clone()));
        }
        if let Some(ids) = &req.id_list {
            params.push(("id_list", ids.clone()));
        }
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

        checked_feed(&xml)
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
        validate_window(req.start, req.max_results)?;
        let mut params: Vec<(&str, String)> = vec![("id_list", req.id_list.clone())];
        if let Some(n) = req.max_results {
            params.push(("max_results", n.to_string()));
        }
        if let Some(n) = req.start {
            params.push(("start", n.to_string()));
        }

        let ref_params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let xml = self.get_atom(&ref_params).await?;

        checked_feed(&xml)
    }
}
fn validate_window(start: Option<u32>, max_results: Option<u32>) -> Result<()> {
    let start = start.unwrap_or(0);
    let size = max_results.unwrap_or(10);
    if size > 2000 || start >= 30000 || start.checked_add(size).is_none_or(|end| end > 30000) {
        return Err(ArxivError::Param("arXiv requests must stay within the first 30000 results and use at most 2000 records per page".into()));
    }
    Ok(())
}
fn checked_feed(xml: &str) -> Result<SearchResponse> {
    let response = crate::convert::parse_feed(xml)?;
    if let Some(entry) = response
        .entries
        .iter()
        .find(|entry| entry.arxiv_id.starts_with("api/errors"))
    {
        return Err(ArxivError::Api(
            entry
                .summary
                .clone()
                .unwrap_or_else(|| entry.arxiv_id.clone()),
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    const EMPTY: &str = "<feed xmlns='http://www.w3.org/2005/Atom' xmlns:os='http://a9.com/-/spec/opensearch/1.1/'><os:totalResults>0</os:totalResults><os:startIndex>0</os:startIndex><os:itemsPerPage>0</os:itemsPerPage></feed>";
    async fn head(stream: &mut TcpStream) {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        while !bytes.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() < 65536);
        }
    }
    async fn reply(stream: &mut TcpStream) {
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{EMPTY}",
                    EMPTY.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn concurrent_calls_share_actual_start_spacing() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Arc::new(
            ArxivClient::new().with_base_url(format!("http://{}", listener.local_addr().unwrap())),
        );
        let server = tokio::spawn(async move {
            let mut starts = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                head(&mut stream).await;
                starts.push(Instant::now());
                reply(&mut stream).await;
            }
            starts
        });
        let tasks: Vec<_> = (0..3)
            .map(|_| {
                let client = client.clone();
                tokio::spawn(async move {
                    client
                        .search(&SearchRequest::new("all:gene"))
                        .await
                        .unwrap()
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let starts = server.await.unwrap();
        assert!(
            starts
                .windows(2)
                .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(2900)),
            "queued callers must not wake at the same instant"
        );
    }
    #[tokio::test]
    async fn one_connection_is_held_until_the_body_finishes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Arc::new(
            ArxivClient::new().with_base_url(format!("http://{}", listener.local_addr().unwrap())),
        );
        let first = {
            let client = client.clone();
            tokio::spawn(
                async move { client.search(&SearchRequest::new("all:one")).await.unwrap() },
            )
        };
        let (mut stream, _) = listener.accept().await.unwrap();
        head(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    EMPTY.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let second =
            tokio::spawn(
                async move { client.search(&SearchRequest::new("all:two")).await.unwrap() },
            );
        tokio::time::sleep(Duration::from_millis(3100)).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "a second connection must not overlap the first response body"
        );
        stream.write_all(EMPTY.as_bytes()).await.unwrap();
        drop(stream);
        let (mut second_stream, _) =
            tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
        head(&mut second_stream).await;
        reply(&mut second_stream).await;
        first.await.unwrap();
        second.await.unwrap();
    }
    #[tokio::test]
    async fn retry_after_dates_and_oversize_headers_are_preserved_without_reading_bodies() {
        for (headers, expected) in [
            ("Retry-After: 60\r\n".to_owned(), Some(60)),
            (
                format!(
                    "Retry-After: {}\r\n",
                    httpdate::fmt_http_date(
                        std::time::SystemTime::now() + Duration::from_secs(120)
                    )
                ),
                None,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = ArxivClient::new()
                .with_base_url(format!("http://{}", listener.local_addr().unwrap()));
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                head(&mut stream).await;
                stream.write_all(format!("HTTP/1.1 429 Too Many Requests\r\n{headers}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            });
            let ArxivError::RateLimited {
                retry_after_seconds: Some(seconds),
            } = client
                .search(&SearchRequest::new("all:gene"))
                .await
                .unwrap_err()
            else {
                panic!("Retry-After missing")
            };
            if let Some(expected) = expected {
                assert_eq!(seconds, expected)
            } else {
                assert!((118..=121).contains(&seconds))
            }
            server.await.unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            ArxivClient::new().with_base_url(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            head(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 20000000\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });
        assert!(matches!(
            client.search(&SearchRequest::new("all:gene")).await,
            Err(ArxivError::ResponseTooLarge)
        ));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn large_chunked_responses_are_bounded_even_without_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            ArxivClient::new().with_base_url(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            head(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let chunk = vec![b'x'; 1024 * 1024];
            for _ in 0..17 {
                if stream.write_all(b"100000\r\n").await.is_err()
                    || stream.write_all(&chunk).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    break;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        });
        assert!(matches!(
            client.search(&SearchRequest::new("all:gene")).await,
            Err(ArxivError::ResponseTooLarge)
        ));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn long_queries_use_post_form_without_changing_native_parameters() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = ArxivClient::new().with_base_url(format!(
            "http://{}/api/query",
            listener.local_addr().unwrap()
        ));
        let query = "all:gene AND ".repeat(800) + "all:therapy";
        let expected = query.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            let boundary = loop {
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    break at + 4;
                }
            };
            let head = String::from_utf8(bytes[..boundary].to_vec()).unwrap();
            assert!(head.starts_with("POST /api/query HTTP/1.1"));
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse().unwrap())
                })
                .unwrap();
            while bytes.len() < boundary + length {
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
            }
            let body = String::from_utf8(bytes[boundary..boundary + length].to_vec()).unwrap();
            let form = reqwest::Url::parse(&format!("http://form/?{body}")).unwrap();
            assert!(
                form.query_pairs()
                    .any(|(key, value)| key == "search_query" && value == expected)
            );
            assert!(
                form.query_pairs()
                    .any(|(key, value)| key == "max_results" && value == "1")
            );
            reply(&mut stream).await;
        });
        assert_eq!(
            client
                .search(&SearchRequest::new(query).max_results(1))
                .await
                .unwrap()
                .total_results,
            0
        );
        server.await.unwrap();
    }
    #[tokio::test]
    async fn invalid_windows_are_rejected_before_attempting_http() {
        let client = ArxivClient::new().with_base_url("http://127.0.0.1:1");
        for request in [
            SearchRequest::new("all:gene").max_results(2001),
            SearchRequest::new("all:gene").start(30000),
            SearchRequest::new("all:gene").start(29999).max_results(2),
        ] {
            assert!(matches!(
                client.search(&request).await,
                Err(ArxivError::Param(_))
            ));
        }
        let mut invalid = FetchRequest::new("2401.12345");
        invalid.max_results = Some(2001);
        assert!(matches!(
            client.fetch_by_id(&invalid).await,
            Err(ArxivError::Param(_))
        ));
    }
    #[test]
    fn malformed_atom_and_http_200_error_entries_never_become_zero_results() {
        for body in [
            "",
            "<html>upstream unavailable</html>",
            "<feed></feed>",
            "<feed><totalResults>0</totalResults>",
        ] {
            assert!(checked_feed(body).is_err(), "{body}");
        }
        let error = "<feed xmlns='http://www.w3.org/2005/Atom'><entry><id>https://arxiv.org/api/errors#bad_query</id><title>Error</title><summary>bad input</summary></entry></feed>";
        assert!(matches!(checked_feed(error), Err(ArxivError::Api(_))));
        assert_eq!(checked_feed(EMPTY).unwrap().total_results, 0);
        for body in [
            format!("unavailable{EMPTY}"),
            format!("{EMPTY}<error/>"),
            EMPTY.replace("http://www.w3.org/2005/Atom", "urn:not-atom"),
            EMPTY
                .replace("<os:totalResults>", "<container><os:totalResults>")
                .replace("</os:itemsPerPage>", "</os:itemsPerPage></container>"),
        ] {
            assert!(checked_feed(&body).is_err());
        }
        let prefixed = EMPTY
            .replace("<feed xmlns=", "<a:feed xmlns:a=")
            .replace("</feed>", "</a:feed>");
        assert_eq!(checked_feed(&prefixed).unwrap().total_results, 0);
    }
}
