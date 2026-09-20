//! `source_nhanes_download` — validated NHANES XPT download.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::FileRef;

/// Spec for `source_nhanes_download`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct NhanesDownloadSpec {
    /// Explicit XPT href from a `source_nhanes_files` listing, e.g.
    /// `/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.xpt`. Mutually
    /// exclusive with `component`/`file_pattern`.
    #[serde(default)]
    pub href: Option<String>,
    /// Convenience mode: component to re-list when `href` is not known.
    /// Requires `file_pattern`.
    #[serde(default)]
    pub component: Option<String>,
    /// Cycle for convenience mode, e.g. `"2017-2018"`. Omit to search all
    /// cycles of the component.
    #[serde(default)]
    pub cycle: Option<String>,
    /// Convenience mode: case-insensitive substring matched against file
    /// stems (e.g. `"DEMO_J"`). Must match exactly one file.
    #[serde(default)]
    pub file_pattern: Option<String>,
    /// Destination `vfs://...` engine storage path or absolute local path.
    /// Must end with `.xpt`.
    pub path: String,
    /// Endpoint override for tests and mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Download one NHANES XPT file with content validation.
#[derive(Clone)]
pub struct NhanesDownloadNode {
    meta: NodePorts,
    spec: NhanesDownloadSpec,
}

pub struct NhanesDownloadNodeFactory;

impl NodeFactory for NhanesDownloadNodeFactory {
    fn kind(&self) -> &'static str {
        "source_nhanes_download"
    }

    fn desc(&self) -> &'static str {
        "Download one NHANES XPT file and emit a FileRef."
    }

    fn doc(&self) -> &'static str {
        "Downloads an NHANES `.xpt` file and writes it to `path`, emitting a \
         FileRef for file_to_dataframe (format `xpt`). Either pass an explicit \
         `href` from source_nhanes_files, or `component` + `file_pattern` to \
         re-list and resolve by file stem (must match exactly one file). \
         Downloads are validated: content-type must not be HTML and the body \
         must start with the XPORT header, so CDC's HTTP-200 error pages \
         never land on disk as data."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(NhanesDownloadSpec)
    }

    fn ports(&self) -> NodePorts {
        super::util::file_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(NhanesDownloadNode {
            meta: super::util::file_port(),
            spec,
        }))
    }
}

fn href_shape_error(href: &str) -> Option<&'static str> {
    let lower = href.to_ascii_lowercase();
    if !(href.starts_with('/') || lower.starts_with("http://") || lower.starts_with("https://")) {
        return Some("must be root-relative (/Nchs/Data/...) or an absolute http(s) URL");
    }
    if !lower.ends_with(".xpt") {
        return Some("must end with .xpt");
    }
    None
}

fn cycle_begin_year(cycle: &str) -> Result<u16, DagError> {
    let digits: String = cycle.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() != 4 {
        return Err(DagError::Schedule(format!(
            "source_nhanes_download: cycle {cycle:?} must start with a four-digit year, e.g. \"2017-2018\""
        )));
    }
    digits
        .parse::<u16>()
        .map_err(|e| DagError::Schedule(format!("source_nhanes_download: invalid cycle year: {e}")))
}

impl NhanesDownloadNode {
    /// Resolve the target href in convenience mode by re-listing the
    /// component and requiring `file_pattern` to match exactly one stem.
    async fn resolve_href(&self, client: &crate::NhanesClient) -> Result<String, DagError> {
        let component = self.spec.component.as_deref().map(str::trim).unwrap_or("");
        let pattern = self
            .spec
            .file_pattern
            .as_deref()
            .map(str::trim)
            .unwrap_or("");
        let year = match self.spec.cycle.as_deref() {
            Some(cycle) => Some(cycle_begin_year(cycle.trim())?),
            None => None,
        };
        let html = client
            .listing_html(component, year)
            .await
            .map_err(|e| DagError::Schedule(format!("NHANES listing request failed: {e}")))?;
        if crate::is_soft_404(&html) {
            return Err(DagError::Schedule(format!(
                "source_nhanes_download: NHANES returned its \"Page Not Found\" template for \
                 component {component:?} (cycle {:?}); check the component spelling \
                 (Demographics, Dietary, Examination, Laboratory, Questionnaire)",
                self.spec.cycle
            )));
        }
        let links = crate::parse_listing(&html);
        let needle = pattern.to_ascii_lowercase();
        let matches: Vec<_> = links
            .iter()
            .filter(|link| link.file_stem.to_ascii_lowercase().contains(&needle))
            .collect();
        let stems = || {
            links
                .iter()
                .map(|link| link.file_stem.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match matches.as_slice() {
            [only] => Ok(only.href.clone()),
            [] => Err(DagError::Schedule(format!(
                "source_nhanes_download: file_pattern {pattern:?} matched no file in component \
                 {component:?}; available stems: {}",
                stems()
            ))),
            many => Err(DagError::Schedule(format!(
                "source_nhanes_download: file_pattern {pattern:?} matched {} files ({}) — make it \
                 more specific, e.g. include the cycle suffix; all stems: {}",
                many.len(),
                many.iter()
                    .map(|link| link.file_stem.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                stems()
            ))),
        }
    }
}

#[async_trait]
impl DagNode for NhanesDownloadNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_nhanes_download"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let path = self.spec.path.trim();
        if path.is_empty() {
            return Err(DagError::Schedule(
                "source_nhanes_download requires `path`".into(),
            ));
        }
        if !path.to_ascii_lowercase().ends_with(".xpt") {
            return Err(DagError::Schedule(format!(
                "source_nhanes_download: path {path:?} must end with .xpt"
            )));
        }

        let convenience = self.spec.component.is_some() || self.spec.file_pattern.is_some();
        if self.spec.href.is_some() && convenience {
            return Err(DagError::Schedule(
                "source_nhanes_download: `href` and `component`/`file_pattern` are mutually \
                 exclusive — pass either an explicit href or a component + file_pattern"
                    .into(),
            ));
        }
        let href = match self.spec.href.as_deref().map(str::trim) {
            Some(href) if !href.is_empty() => {
                if let Some(problem) = href_shape_error(href) {
                    return Err(DagError::Schedule(format!(
                        "source_nhanes_download: href {href:?} {problem}"
                    )));
                }
                href.to_string()
            }
            _ => {
                if !convenience {
                    return Err(DagError::Schedule(
                        "source_nhanes_download requires either `href` or \
                         `component` + `file_pattern`"
                            .into(),
                    ));
                }
                if self
                    .spec
                    .component
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
                {
                    return Err(DagError::Schedule(
                        "source_nhanes_download: convenience mode requires a non-empty \
                         `component`"
                            .into(),
                    ));
                }
                let client = match self.spec.endpoint.as_deref() {
                    Some(url) => crate::NhanesClient::with_endpoint(url),
                    None => crate::NhanesClient::new(),
                };
                self.resolve_href(&client).await?
            }
        };

        let client = match self.spec.endpoint.as_deref() {
            Some(url) => crate::NhanesClient::with_endpoint(url),
            None => crate::NhanesClient::new(),
        };
        let bytes = client
            .download_xpt(&href)
            .await
            .map_err(|e| DagError::Schedule(format!("NHANES download failed: {e}")))?;
        let file_ref = super::util::write_output(ctx, path, "xpt", bytes).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file_ref);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use dag_core::dag::node_event::NodeReporter;
    use dag_core::value::NodeValue;
    use datafusion::prelude::SessionContext;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_stub(responses: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let responses: VecDeque<_> = responses.into_iter().collect();
        tokio::spawn(async move {
            for (content_type, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    let read = socket.read(&mut buffer).await.unwrap();
                    request.extend_from_slice(&buffer[..read]);
                    if read == 0 || request.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        endpoint
    }

    fn ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    fn file_ref(outputs: &PortOutputs) -> FileRef {
        match outputs.get(&0) {
            Some(NodeValue::File(file)) => file.clone(),
            other => panic!("expected a File output, got {other:?}"),
        }
    }

    // Minimal XPORT body: the magic prefix download_xpt validates.
    fn xpt_body() -> Vec<u8> {
        let mut body = b"HEADER RECORD*******LIBRARY HEADER RECORD!!!!!!!".to_vec();
        body.extend_from_slice(b"000000000000000001600000000140+2026-09-19 00:00:00");
        body
    }

    /// Single-cycle listing (what a cycle-scoped convenience-mode request
    /// really gets back): rows have no Years column.
    const SINGLE_CYCLE_HTML: &str = r#"
<table>
<tr>
    <td class="text-left">Demographic Variables and Sample Weights</td>
    <td class="text-center"><a href="/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.htm">DEMO_J Doc</a></td>
    <td class="text-center"><a href="/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.xpt">DEMO_J Data [XPT - 3.3 MB]</a></td>
    <td class="text-center">February 2020</td>
</tr>
</table>
"#;

    const LISTING_HTML: &str = r#"
<table>
<tr>
    <td>2017-2018</td>
    <td>Demographics</td>
    <td><a href="/Nchs/Nhanes/2017/DEMO_J.htm">Doc</a></td>
    <td><a href="/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.xpt">[XPT - 41.7 MB]</a></td>
    <td>Sept 2019</td>
</tr>
<tr>
    <td>2015-2016</td>
    <td>Demographics</td>
    <td><a href="/Nchs/Nhanes/2015/DEMO_I.htm">Doc</a></td>
    <td><a href="/Nchs/Data/Nhanes/Public/2015/DataFiles/DEMO_I.xpt">[XPT - 40.1 MB]</a></td>
    <td>Oct 2017</td>
</tr>
</table>
"#;

    #[tokio::test]
    async fn explicit_href_downloads_validated_bytes() {
        let body = xpt_body();
        let endpoint = spawn_stub(vec![("text/plain", body.clone())]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo_j.xpt");
        let path_str = path.to_str().unwrap().to_owned();
        let ctx = ctx();
        let spec = serde_json::json!({
            "href": "/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.xpt",
            "path": path_str,
            "endpoint": endpoint
        });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let file = file_ref(&outputs);
        assert_eq!(file.path, path_str);
        assert_eq!(file.format.as_deref(), Some("xpt"));
        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(on_disk, body);
        assert_eq!(node.sink_path(), Some(path_str.as_str()));
    }

    #[tokio::test]
    async fn html_error_page_is_rejected() {
        let endpoint = spawn_stub(vec![(
            "text/html",
            b"<html><head><title>Runtime Error</title></head></html>".to_vec(),
        )])
        .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo_j.xpt");
        let ctx = ctx();
        let spec = serde_json::json!({
            "href": "/Nchs/Data/Nhanes/Public/2017/DataFiles/GONE_J.xpt",
            "path": path.to_str().unwrap(),
            "endpoint": endpoint
        });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("HTTP 200"), "{message}");
        assert!(message.contains("text/html"), "{message}");
        assert!(message.contains("Runtime Error"), "{message}");
        assert!(!path.exists(), "rejected body must not land on disk");
    }

    #[tokio::test]
    async fn convenience_mode_requires_a_unique_stem() {
        let body = xpt_body();
        let endpoint = spawn_stub(vec![
            ("text/html", SINGLE_CYCLE_HTML.to_string().into_bytes()),
            ("text/plain", body.clone()),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.xpt");
        let ctx = ctx();
        let spec = serde_json::json!({
            "component": "Demographics",
            "cycle": "2017-2018",
            "file_pattern": "DEMO_J",
            "path": path.to_str().unwrap(),
            "endpoint": endpoint
        });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        assert_eq!(file_ref(&outputs).format.as_deref(), Some("xpt"));
        assert_eq!(std::fs::read(&path).unwrap(), body);
    }

    #[tokio::test]
    async fn ambiguous_and_missing_patterns_list_candidates() {
        // "DEMO" matches both DEMO_J and DEMO_I → ambiguous.
        let endpoint = spawn_stub(vec![("text/html", LISTING_HTML.to_string().into_bytes())]).await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx();
        let spec = serde_json::json!({
            "component": "Demographics",
            "file_pattern": "DEMO",
            "path": dir.path().join("demo.xpt"),
            "endpoint": endpoint
        });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("matched 2 files"), "{message}");
        assert!(message.contains("DEMO_J, DEMO_I"), "{message}");

        // "NOPE" matches nothing → zero-hit error lists available stems.
        let endpoint = spawn_stub(vec![("text/html", LISTING_HTML.to_string().into_bytes())]).await;
        let spec = serde_json::json!({
            "component": "Demographics",
            "file_pattern": "NOPE",
            "path": dir.path().join("demo.xpt"),
            "endpoint": endpoint
        });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("matched no file"), "{message}");
        assert!(message.contains("DEMO_J"), "{message}");
    }

    #[tokio::test]
    async fn mode_and_shape_validation() {
        let ctx = ctx();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.xpt");

        // href + component are mutually exclusive.
        let spec = serde_json::json!({
            "href": "/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.xpt",
            "component": "Demographics",
            "path": path.to_str().unwrap()
        });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("mutually exclusive"));

        // Neither mode given.
        let spec = serde_json::json!({ "path": path.to_str().unwrap() });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires either"));

        // href must be .xpt.
        let spec = serde_json::json!({
            "href": "/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.htm",
            "path": path.to_str().unwrap()
        });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("must end with .xpt"));

        // path must be .xpt.
        let spec = serde_json::json!({
            "href": "/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.xpt",
            "path": dir.path().join("demo.csv")
        });
        let mut node = NhanesDownloadNodeFactory.build(spec, ctx.clone()).unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("path"));
    }
}
