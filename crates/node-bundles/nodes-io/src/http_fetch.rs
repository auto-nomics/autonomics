//! Allowlist-gated HTTP fetch node.
//!
//! The node downloads a single URL into a File output, but only when the
//! URL's host is listed in the network allowlist — a deny-by-default guard so
//! a runaway workflow cannot exfiltrate or pull arbitrary hosts. The
//! allowlist lives in `state_dir/network-allowlist.toml`; the gateway daemon
//! creates a commented template there on startup and points
//! [`ENV_NETWORK_ALLOWLIST`] at it. An allowlist entry matches its own host
//! and every subdomain (`ebi.ac.uk` also covers `ftp.ebi.ac.uk`).

use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::value::{FileRef, PortType};
use dag_core::{
    dag::DagError,
    dag::graph::PortOutputs,
    registry::{NodeCtx, NodeFactory},
};

pub const HTTP_FETCH_KIND: &str = "http_fetch";
/// Environment variable holding the allowlist file path. Set by the gateway
/// daemon (`serve`) to `<state_dir>/network-allowlist.toml`.
pub const ENV_NETWORK_ALLOWLIST: &str = "AUTONOMICS_NETWORK_ALLOWLIST";
pub const ALLOWLIST_FILE_NAME: &str = "network-allowlist.toml";

const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum HttpFetchError {
    #[error("invalid http_fetch spec: {message}")]
    InvalidSpec { message: String },

    #[error(
        "no HTTP fetch allowlist is configured ({reason}); the gateway daemon creates \
         `<state_dir>/{ALLOWLIST_FILE_NAME}` and points {ENV_NETWORK_ALLOWLIST} at it on startup"
    )]
    AllowlistUnavailable { reason: String },

    #[error("cannot read HTTP fetch allowlist `{path}`: {source}")]
    AllowlistRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid HTTP fetch allowlist `{path}`: {reason}")]
    AllowlistParse { path: String, reason: String },

    #[error(
        "host `{host}` is not on the HTTP fetch allowlist; add `[[allow]]` with \
         `host = \"{host}\"` to the file named by {ENV_NETWORK_ALLOWLIST} and restart the \
         gateway, or remove this node from the workflow"
    )]
    DomainNotAllowed { host: String },

    #[error("HTTP request to `{url}` failed: {source}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("redirect from `{url}` was refused: {reason}")]
    RedirectRefused { url: String, reason: String },

    #[error("HTTP `{url}` answered {status}")]
    HttpStatus {
        url: String,
        status: reqwest::StatusCode,
    },

    #[error("`{url}` body exceeds http_fetch max_bytes ({max_bytes})")]
    TooLarge { url: String, max_bytes: u64 },

    #[error("cannot write fetched body to `{path}`: {reason}")]
    Write { path: String, reason: String },
}

impl ::dag_core::dag::NodeError for HttpFetchError {
    fn node_type(&self) -> &str {
        "http_fetch"
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct HttpFetchSpec {
    /// Absolute http(s) URL to download.
    url: String,
    /// Output path for the response body (`/…`, `file://…`, or `vfs://…`).
    path: String,
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
    /// Refuse bodies larger than this many bytes (buffered in memory).
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_max_bytes() -> u64 {
    DEFAULT_MAX_BYTES
}

#[derive(Clone)]
pub struct HttpFetchNode {
    meta: NodePorts,
    spec: HttpFetchSpec,
}

pub struct HttpFetchNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type(None, PortType::File)
}

// ── Allowlist parsing and matching ─────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct AllowlistFile {
    #[serde(default, rename = "allow")]
    allow: Vec<AllowEntry>,
}

#[derive(Debug, Deserialize)]
struct AllowEntry {
    host: String,
}

/// Commented template written to `state_dir/network-allowlist.toml` on first
/// daemon start (deny-by-default: nothing is fetchable until a host is added).
pub fn default_allowlist_toml() -> &'static str {
    r#"# HTTP fetch allowlist for the `http_fetch` DAG node.
#
# Deny-by-default: a fetch is refused unless the URL host matches an entry
# below. An entry matches its own host and any subdomain — `ebi.ac.uk` also
# covers `ftp.ebi.ac.uk`. Entries are hostnames only (no scheme, no path).
# Restart the gateway after editing.

# [[allow]]
# host = "ftp.ebi.ac.uk"
"#
}

/// Parse the allowlist TOML into normalized host entries.
pub fn parse_allowlist(path: &str, source: &str) -> Result<Vec<String>, HttpFetchError> {
    let parsed: AllowlistFile =
        toml::from_str(source).map_err(|error| HttpFetchError::AllowlistParse {
            path: path.to_string(),
            reason: error.to_string(),
        })?;
    let mut entries = BTreeSet::new();
    for entry in parsed.allow {
        let host = entry.host.trim().to_ascii_lowercase();
        if host.is_empty() || host.contains("://") || host.contains('/') || host.contains(' ') {
            return Err(HttpFetchError::AllowlistParse {
                path: path.to_string(),
                reason: format!("invalid allow entry host `{}`", entry.host),
            });
        }
        entries.insert(host.trim_matches('.').to_string());
    }
    Ok(entries.into_iter().collect())
}

/// Load the allowlist from the path named by [`ENV_NETWORK_ALLOWLIST`].
fn load_allowlist_from_env() -> Result<Vec<String>, HttpFetchError> {
    let Some(path) = std::env::var(ENV_NETWORK_ALLOWLIST)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(HttpFetchError::AllowlistUnavailable {
            reason: format!("{ENV_NETWORK_ALLOWLIST} is not set"),
        });
    };
    let source =
        std::fs::read_to_string(&path).map_err(|source| HttpFetchError::AllowlistRead {
            path: path.clone(),
            source,
        })?;
    parse_allowlist(&path, &source)
}

/// Suffix host match: an entry covers itself and any subdomain.
pub fn host_allowed(host: &str, entries: &[String]) -> bool {
    let host = host.trim_matches('.').to_ascii_lowercase();
    entries
        .iter()
        .any(|entry| host == *entry || host.ends_with(&format!(".{entry}")))
}

fn validate_spec(spec: &HttpFetchSpec) -> Result<(), HttpFetchError> {
    if spec.timeout_secs == 0 {
        return Err(HttpFetchError::InvalidSpec {
            message: "timeout_secs must be greater than zero".into(),
        });
    }
    if spec.max_bytes == 0 {
        return Err(HttpFetchError::InvalidSpec {
            message: "max_bytes must be greater than zero".into(),
        });
    }
    let url = parse_url(&spec.url)?;
    if spec.path.trim().is_empty() {
        return Err(HttpFetchError::InvalidSpec {
            message: "path cannot be empty".into(),
        });
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(HttpFetchError::InvalidSpec {
            message: format!("URL `{}` has no host", spec.url),
        });
    }
    Ok(())
}

fn parse_url(url: &str) -> Result<reqwest::Url, HttpFetchError> {
    let parsed = reqwest::Url::parse(url).map_err(|error| HttpFetchError::InvalidSpec {
        message: format!("cannot parse URL `{url}`: {error}"),
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(HttpFetchError::InvalidSpec {
            message: format!(
                "unsupported scheme `{}` in `{url}`; only http and https are allowed",
                parsed.scheme()
            ),
        });
    }
    Ok(parsed)
}

/// Redirects are followed only while every hop stays on the allowlist.
fn redirect_policy(entries: Vec<String>) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > 5 {
            return attempt.error("too many redirects");
        }
        match attempt.url().host_str() {
            Some(host) if host_allowed(host, &entries) => attempt.follow(),
            _ => attempt.error("redirect target host is not on the HTTP fetch allowlist"),
        }
    })
}

/// reqwest's own Display for redirect failures hides the policy message (it
/// lives in the source chain); surface it so refusals explain themselves.
fn describe_request_error(url: &str, source: reqwest::Error) -> HttpFetchError {
    if source.is_redirect() {
        let reason = std::error::Error::source(&source)
            .map(|cause| cause.to_string())
            .unwrap_or_else(|| "no reason given".to_string());
        return HttpFetchError::RedirectRefused {
            url: url.to_string(),
            reason,
        };
    }
    HttpFetchError::Request {
        url: url.to_string(),
        source,
    }
}

async fn write_body(node_ctx: &NodeCtx, path: &str, bytes: Vec<u8>) -> Result<(), HttpFetchError> {
    if let Some(virtual_path) = path.strip_prefix("vfs://") {
        let storage = node_ctx
            .opendal
            .as_ref()
            .ok_or_else(|| HttpFetchError::Write {
                path: path.to_string(),
                reason: "vfs:// output requires mounted object storage".into(),
            })?;
        let operator = storage.resolve(virtual_path);
        operator
            .write(&storage.resolve_path(virtual_path), bytes)
            .await
            .map_err(|error| HttpFetchError::Write {
                path: path.to_string(),
                reason: error.to_string(),
            })?;
        return Ok(());
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| HttpFetchError::Write {
                path: path.to_string(),
                reason: format!("cannot create `{}`: {error}", parent.display()),
            })?;
    }
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| HttpFetchError::Write {
            path: path.to_string(),
            reason: error.to_string(),
        })
}

fn file_output(path: &str) -> FileRef {
    let format = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    if path.starts_with("vfs://") {
        return FileRef::new(path.to_string(), format);
    }
    FileRef::local(path, format).unwrap_or_else(|_| FileRef::new(path.to_string(), None))
}

impl HttpFetchNode {
    pub fn new(spec: HttpFetchSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
        }
    }

    /// Execute against an explicit allowlist instead of the environment
    /// variable (test seam; `None` behaves like an unset variable).
    async fn execute_with_allowlist(
        &mut self,
        node_ctx: &NodeCtx,
        allowlist: Option<Vec<String>>,
    ) -> Result<PortOutputs, HttpFetchError> {
        validate_spec(&self.spec)?;
        let entries = match allowlist {
            Some(entries) => entries,
            None => load_allowlist_from_env()?,
        };
        let url = parse_url(&self.spec.url)?;
        let host = url.host_str().unwrap_or_default().to_string();
        if !host_allowed(&host, &entries) {
            return Err(HttpFetchError::DomainNotAllowed { host });
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.spec.timeout_secs))
            .redirect(redirect_policy(entries))
            .build()
            .map_err(|source| describe_request_error(&self.spec.url, source))?;
        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|source| describe_request_error(&self.spec.url, source))?;
        let status = response.status();
        if !status.is_success() {
            return Err(HttpFetchError::HttpStatus {
                url: self.spec.url.clone(),
                status,
            });
        }
        if let Some(length) = response.content_length()
            && length > self.spec.max_bytes
        {
            return Err(HttpFetchError::TooLarge {
                url: self.spec.url.clone(),
                max_bytes: self.spec.max_bytes,
            });
        }

        let mut body: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|source| describe_request_error(&self.spec.url, source))?;
            if body.len() as u64 + chunk.len() as u64 > self.spec.max_bytes {
                return Err(HttpFetchError::TooLarge {
                    url: self.spec.url.clone(),
                    max_bytes: self.spec.max_bytes,
                });
            }
            body.extend_from_slice(&chunk);
        }

        let path = crate::file_to_dataframe::source_path(
            node_ctx,
            &crate::file_to_dataframe::normalize_path(&self.spec.path),
        );
        write_body(node_ctx, &path, body).await?;
        let file = file_output(&path);
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

#[async_trait]
impl DagNode for HttpFetchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        HTTP_FETCH_KIND
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        self.execute_with_allowlist(node_ctx, None)
            .await
            .map_err(Into::into)
    }
}

impl NodeFactory for HttpFetchNodeFactory {
    fn kind(&self) -> &'static str {
        HTTP_FETCH_KIND
    }

    fn desc(&self) -> &'static str {
        "Downloads an allowlisted http(s) URL into a File output."
    }

    fn doc(&self) -> &'static str {
        "No input ports; one File output holding the response body. The URL host must be \
        on the network allowlist (deny-by-default): the gateway daemon creates \
        state_dir/network-allowlist.toml on first start and points the \
        AUTONOMICS_NETWORK_ALLOWLIST environment variable at it. An entry matches its own \
        host and any subdomain. Redirects must stay on the allowlist at every hop; bodies \
        larger than max_bytes are refused; the body is buffered in memory."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HttpFetchSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: HttpFetchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(HttpFetchNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(url: &str, path: &str) -> HttpFetchSpec {
        HttpFetchSpec {
            url: url.to_string(),
            path: path.to_string(),
            timeout_secs: 10,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    fn file_output_of(outputs: &PortOutputs) -> &FileRef {
        match outputs.get(&0).unwrap() {
            dag_core::value::NodeValue::File(file) => file,
            other => panic!("expected a File output, got {other:?}"),
        }
    }

    #[test]
    fn parse_allowlist_accepts_entries_and_rejects_bad_hosts() {
        let path = "/tmp/allowlist.toml";
        let entries = parse_allowlist(path, "[[allow]]\nhost = \"FTP.EBI.ac.uk\"\n").unwrap();
        assert_eq!(entries, vec!["ftp.ebi.ac.uk"]);
        assert!(parse_allowlist(path, "").unwrap().is_empty());

        let error = parse_allowlist(path, "[[allow]]\nhost = \"https://x.com/a\"\n").unwrap_err();
        assert!(
            error.to_string().contains("invalid allow entry host"),
            "{error}"
        );
        let error = parse_allowlist(path, "allow = [oops]\n").unwrap_err();
        assert!(
            error.to_string().contains("invalid HTTP fetch allowlist"),
            "{error}"
        );
    }

    #[test]
    fn host_matching_covers_subdomains_only() {
        let entries = vec!["ebi.ac.uk".to_string(), "localhost".to_string()];
        assert!(host_allowed("ebi.ac.uk", &entries));
        assert!(host_allowed("ftp.ebi.ac.uk", &entries));
        assert!(host_allowed("FTP.EBI.AC.UK", &entries));
        assert!(host_allowed("localhost", &entries));
        assert!(host_allowed("127.0.0.1", &["127.0.0.1".into()]));
        assert!(!host_allowed("notebi.ac.uk", &entries));
        assert!(!host_allowed("evil-ebi.ac.uk", &entries));
        assert!(!host_allowed("ac.uk", &entries));
        assert!(!host_allowed("example.com", &entries));
    }

    #[test]
    fn url_validation_rejects_non_http_schemes_and_missing_hosts() {
        assert!(validate_spec(&spec("ftp://ebi.ac.uk/file", "/out.bin")).is_err());
        assert!(validate_spec(&spec("file:///etc/passwd", "/out.bin")).is_err());
        // `http:///x` collapses to host `x` per WHATWG; a truly empty host
        // fails at parse time instead.
        assert!(validate_spec(&spec("http:///", "/out.bin")).is_err());
        assert!(validate_spec(&spec("https://ebi.ac.uk/f", " ")).is_err());
        assert!(validate_spec(&spec("https://ebi.ac.uk/f?q=1", "/dir/out.tsv.gz")).is_ok());
        let mut zero = spec("https://ebi.ac.uk/", "/out");
        zero.timeout_secs = 0;
        assert!(validate_spec(&zero).is_err());
        zero.timeout_secs = 10;
        zero.max_bytes = 0;
        assert!(validate_spec(&zero).is_err());
    }

    #[tokio::test]
    async fn fetch_fails_without_an_allowlist() {
        let mut node = HttpFetchNode::new(spec("https://ebi.ac.uk/x", "/tmp/http_fetch_out"));
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let error = node.execute_with_allowlist(&ctx, None).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no HTTP fetch allowlist is configured"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn fetch_refuses_hosts_off_the_allowlist() {
        let mut node =
            HttpFetchNode::new(spec("https://example.org/data.tsv", "/tmp/http_fetch_out"));
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let error = node
            .execute_with_allowlist(&ctx, Some(vec!["ebi.ac.uk".into()]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("example.org"), "{error}");
        assert!(
            error
                .to_string()
                .contains("not on the HTTP fetch allowlist"),
            "{error}"
        );
    }

    /// Bind 127.0.0.1:0 and answer exactly one request with `response`.
    async fn one_shot_server(response: &'static [u8]) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 4096];
            let _ = socket.read(&mut buffer).await;
            let _ = socket.write_all(response).await;
        });
        port
    }

    #[tokio::test]
    async fn fetch_downloads_an_allowlisted_host_into_the_output_file() {
        let port = one_shot_server(
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
        )
        .await;
        let out = tempfile::tempdir().unwrap().keep().join("body.tsv");
        let mut node = HttpFetchNode::new(spec(
            &format!("http://127.0.0.1:{port}/data.tsv"),
            &out.to_string_lossy(),
        ));
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let outputs = node
            .execute_with_allowlist(&ctx, Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap();
        let file = file_output_of(&outputs);
        assert_eq!(std::fs::read(&file.path).unwrap(), b"hello");
        assert_eq!(file.format.as_deref(), Some("tsv"));
    }

    #[tokio::test]
    async fn redirects_must_stay_on_the_allowlist() {
        // 127.0.0.1 is allowlisted; the redirect hop to `localhost` is not,
        // even though it is the same socket.
        let port = one_shot_server(
            b"HTTP/1.1 302 Found\r\ncontent-length: 0\r\nlocation: http://localhost:9/x\r\nconnection: close\r\n\r\n",
        )
        .await;
        let out = tempfile::tempdir().unwrap().keep().join("body.bin");
        let mut node = HttpFetchNode::new(spec(
            &format!("http://127.0.0.1:{port}/jump"),
            &out.to_string_lossy(),
        ));
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let error = node
            .execute_with_allowlist(&ctx, Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not on the HTTP fetch allowlist"),
            "{error}"
        );
        assert!(
            matches!(error, HttpFetchError::RedirectRefused { .. }),
            "{error}"
        );
        assert!(
            !out.exists(),
            "no body may be written for a refused redirect"
        );
    }

    #[tokio::test]
    async fn oversized_bodies_are_refused() {
        let port = one_shot_server(
            b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\nconnection: close\r\n\r\n0123456789\
              0123456789012345678901234567890123456789012345678901234567890123456789012345678\
              90123456789",
        )
        .await;
        let out = tempfile::tempdir().unwrap().keep().join("big.bin");
        let mut http_spec = spec(
            &format!("http://127.0.0.1:{port}/big.bin"),
            &out.to_string_lossy(),
        );
        http_spec.max_bytes = 50;
        let mut node = HttpFetchNode::new(http_spec);
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let error = node
            .execute_with_allowlist(&ctx, Some(vec!["127.0.0.1".into()]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("max_bytes (50)"), "{error}");
    }
}
