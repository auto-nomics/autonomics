//! PMC's current public article-version metadata, not the retired OA/FTP API.
use crate::error::{Error, Result};
use reqwest::Client;
use serde::Deserialize;

pub const PMC_CLOUD: &str = "https://pmc-oa-opendata.s3.amazonaws.com";
const MAX_PDF_BYTES: usize = 50 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PmcPdf {
    pub pmcid: String,
    pub version: u32,
    pub source_url: String,
    pub bytes: Vec<u8>,
}
impl PmcPdf {
    pub fn filename(&self) -> String {
        format!("{}.{}.pdf", self.pmcid, self.version)
    }
}

#[derive(Deserialize, Default)]
struct Listing {
    #[serde(rename = "CommonPrefixes", default)]
    prefixes: Vec<Prefix>,
    #[serde(rename = "IsTruncated", default)]
    truncated: bool,
    #[serde(rename = "NextContinuationToken")]
    continuation: Option<String>,
}
#[derive(Deserialize)]
struct Prefix {
    #[serde(rename = "Prefix")]
    prefix: String,
}
#[derive(Deserialize)]
struct Metadata {
    pmcid: String,
    version: u32,
    #[serde(default)]
    is_manuscript: bool,
    #[serde(default)]
    is_pmc_openaccess: bool,
    #[serde(default)]
    is_historical_ocr: bool,
    #[serde(default)]
    pdf_url: Option<String>,
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Unknown(message.into())
}

pub fn normalize_pmc_id(value: &str) -> Option<String> {
    let value = value.trim();
    let number = value
        .get(..3)
        .filter(|s| s.eq_ignore_ascii_case("PMC"))
        .map(|_| &value[3..])
        .unwrap_or(value);
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .then(|| format!("PMC{number}"))
}
async fn bounded(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(invalid("PMC response exceeds size limit"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| invalid(e.to_string()))? {
        if bytes.len() + chunk.len() > max {
            return Err(invalid("PMC response exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn get(client: &Client, url: &str, max: usize) -> Result<Option<Vec<u8>>> {
    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(90))
        .send()
        .await
        .map_err(|e| invalid(e.to_string()))?;
    if matches!(response.status().as_u16(), 403 | 404 | 410) {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|e| invalid(e.to_string()))?;
    Ok(Some(bounded(response, max).await?))
}

/// Prefer a published version with a PDF over an author manuscript. Only then
/// use the version number as a deterministic tie-breaker; a larger number does
/// not by itself identify a publisher's version.
pub async fn download(client: &Client, base: &str, pmcid: &str) -> Result<Option<PmcPdf>> {
    let pmcid = normalize_pmc_id(pmcid).ok_or_else(|| invalid("invalid PMCID"))?;
    let base = base.trim_end_matches('/');
    let mut prefixes = Vec::new();
    let mut continuation: Option<String> = None;
    let mut seen_tokens = std::collections::HashSet::new();
    loop {
        let mut url =
            reqwest::Url::parse(&format!("{base}/")).map_err(|e| invalid(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("list-type", "2")
            .append_pair("prefix", &format!("{pmcid}."))
            .append_pair("delimiter", "/");
        if let Some(token) = &continuation {
            url.query_pairs_mut()
                .append_pair("continuation-token", token);
        }
        let Some(xml) = get(client, url.as_str(), MAX_METADATA_BYTES).await? else {
            return Ok(None);
        };
        let listing: Listing = quick_xml::de::from_reader(xml.as_slice())
            .map_err(|e| invalid(format!("invalid PMC object listing: {e}")))?;
        for p in listing.prefixes {
            let name = p.prefix.trim_end_matches('/');
            if name
                .strip_prefix(&format!("{pmcid}."))
                .and_then(|v| v.parse::<u32>().ok())
                .is_some()
            {
                prefixes.push(p.prefix);
            }
        }
        if !listing.truncated {
            break;
        }
        let token = listing
            .continuation
            .ok_or_else(|| invalid("truncated PMC listing has no continuation token"))?;
        if !seen_tokens.insert(token.clone()) || seen_tokens.len() > 32 || prefixes.len() > 1000 {
            return Err(invalid("invalid PMC listing pagination"));
        }
        continuation = Some(token);
    }
    let mut candidates = Vec::new();
    for prefix in prefixes {
        let name = prefix.trim_end_matches('/');
        let url = format!("{base}/{name}/{name}.json");
        let Some(bytes) = get(client, &url, MAX_METADATA_BYTES).await? else {
            continue;
        };
        let m: Metadata = serde_json::from_slice(&bytes)
            .map_err(|e| invalid(format!("invalid PMC metadata: {e}")))?;
        if m.pmcid != pmcid
            || name != format!("{}.{}", m.pmcid, m.version)
            || !(m.is_pmc_openaccess || m.is_manuscript || m.is_historical_ocr)
        {
            continue;
        }
        let Some(pdf) = m.pdf_url.filter(|s| !s.is_empty()) else {
            continue;
        };
        let pdf = if let Some(key) = pdf.strip_prefix("s3://pmc-oa-opendata/") {
            format!("{base}/{key}")
        } else {
            pdf
        };
        let parsed = reqwest::Url::parse(&pdf).map_err(|e| invalid(e.to_string()))?;
        let expected =
            reqwest::Url::parse(&format!("{base}/{name}/")).map_err(|e| invalid(e.to_string()))?;
        if parsed.origin() != expected.origin()
            || !parsed.path().starts_with(expected.path())
            || !parsed.path().ends_with(".pdf")
        {
            return Err(invalid(
                "PMC metadata PDF URL is outside its article-version prefix",
            ));
        }
        candidates.push((m.is_manuscript, m.version, pdf));
    }
    candidates.sort_by_key(|(manuscript, version, _)| (*manuscript, std::cmp::Reverse(*version)));
    for (_, version, url) in candidates {
        let Some(bytes) = get(client, &url, MAX_PDF_BYTES).await? else {
            continue;
        };
        if !bytes.starts_with(b"%PDF-") {
            return Err(invalid("PMC download is not a PDF (missing PDF signature)"));
        }
        return Ok(Some(PmcPdf {
            pmcid,
            version,
            source_url: url,
            bytes,
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Server {
        base: String,
        requests: Arc<Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn server(routes: HashMap<String, String>) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        let routes = Arc::new(routes);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let routes = routes.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buffer = [0; 2048];
                    loop {
                        let n = stream.read(&mut buffer).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                        if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let header = String::from_utf8_lossy(&bytes);
                    let target = header.split_whitespace().nth(1).unwrap().to_owned();
                    log.lock().unwrap().push(target.clone());
                    let path = target.split('?').next().unwrap();
                    let (status, body) = routes
                        .get(path)
                        .map(|s| ("200 OK", s.as_bytes()))
                        .unwrap_or(("404 Not Found", b"missing"));
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                });
            }
        });
        Server {
            base,
            requests,
            task,
        }
    }
    fn listing(versions: &[u32]) -> String {
        format!(
            "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Prefix>PMC42.</Prefix><IsTruncated>false</IsTruncated>{}</ListBucketResult>",
            versions
                .iter()
                .map(|v| format!("<CommonPrefixes><Prefix>PMC42.{v}/</Prefix></CommonPrefixes>"))
                .collect::<String>()
        )
    }
    fn metadata(version: u32, manuscript: bool, pdf: Option<&str>) -> String {
        serde_json::json!({"pmcid":"PMC42","version":version,"is_manuscript":manuscript,"is_pmc_openaccess":true,"pdf_url":pdf}).to_string()
    }
    #[tokio::test]
    async fn discovers_version_prefixes_uses_metadata_pdf_url_and_prefers_published_pdf() {
        let s = server(HashMap::from([
            ("/".into(), listing(&[10, 1])),
            (
                "/PMC42.1/PMC42.1.json".into(),
                metadata(
                    1,
                    false,
                    Some("s3://pmc-oa-opendata/PMC42.1/publisher.pdf?md5=abc"),
                ),
            ),
            (
                "/PMC42.10/PMC42.10.json".into(),
                metadata(
                    10,
                    true,
                    Some("s3://pmc-oa-opendata/PMC42.10/manuscript.pdf"),
                ),
            ),
            (
                "/PMC42.1/publisher.pdf".into(),
                "%PDF-1.7\nPublisher original\n%%EOF".into(),
            ),
        ]))
        .await;
        let pdf = download(&Client::new(), &s.base, "pmc42")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pdf.version, 1);
        assert!(pdf.source_url.ends_with("publisher.pdf?md5=abc"));
        assert!(pdf.bytes.starts_with(b"%PDF-"));
        assert!(
            !s.requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.contains("fullTextXML"))
        );
    }
    #[tokio::test]
    async fn no_pdf_never_falls_back_to_xml_or_text() {
        let s = server(HashMap::from([
            ("/".into(), listing(&[1])),
            ("/PMC42.1/PMC42.1.json".into(), metadata(1, false, None)),
        ]))
        .await;
        assert!(
            download(&Client::new(), &s.base, "PMC42")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(s.requests.lock().unwrap().len(), 2);
    }
    #[tokio::test]
    async fn rejects_html_disguised_as_a_pdf_and_untrusted_metadata_urls() {
        let s = server(HashMap::from([
            ("/".into(), listing(&[1])),
            (
                "/PMC42.1/PMC42.1.json".into(),
                metadata(1, false, Some("s3://pmc-oa-opendata/PMC42.1/article.pdf")),
            ),
            (
                "/PMC42.1/article.pdf".into(),
                "<html>Access denied</html>".into(),
            ),
        ]))
        .await;
        assert!(
            download(&Client::new(), &s.base, "PMC42")
                .await
                .unwrap_err()
                .to_string()
                .contains("not a PDF")
        );
        let s = server(HashMap::from([
            ("/".into(), listing(&[1])),
            (
                "/PMC42.1/PMC42.1.json".into(),
                metadata(1, false, Some("http://127.0.0.1:1/outside.pdf")),
            ),
        ]))
        .await;
        assert!(
            download(&Client::new(), &s.base, "PMC42")
                .await
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
    }
    #[test]
    fn normalizes_only_valid_accessions() {
        assert_eq!(normalize_pmc_id(" pmc42 "), Some("PMC42".into()));
        assert_eq!(normalize_pmc_id("42"), Some("PMC42".into()));
        assert!(normalize_pmc_id("PMC42/../../secret").is_none());
    }
}
