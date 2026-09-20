//! `source_nhanes_files` — NHANES component listing as a table.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::NhanesClient;

/// Components accepted by the `datapage.aspx` listing endpoint.
const COMPONENTS: &str =
    "Demographics, Dietary, Examination, Laboratory, Questionnaire (also: Non-Questionnaire Data)";

fn listing_port() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

fn client(endpoint: Option<&str>) -> NhanesClient {
    match endpoint {
        Some(url) => NhanesClient::with_endpoint(url),
        None => NhanesClient::new(),
    }
}

/// First four-digit year in a cycle label, e.g. `2017` from `"2017-2018"`.
fn cycle_begin_year(cycle: &str) -> Result<u16, DagError> {
    let digits: String = cycle.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() != 4 {
        return Err(DagError::Schedule(format!(
            "source_nhanes_files: cycle {cycle:?} must start with a four-digit year, e.g. \"2017-2018\""
        )));
    }
    digits
        .parse::<u16>()
        .map_err(|e| DagError::Schedule(format!("source_nhanes_files: invalid cycle year: {e}")))
}

/// Spec for `source_nhanes_files`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct NhanesFilesSpec {
    /// NHANES component whose file listing to fetch. Valid components:
    /// `Demographics`, `Dietary`, `Examination`, `Laboratory`, `Questionnaire`.
    pub component: String,
    /// Survey cycle, e.g. `"2017-2018"` (the start year is used). Omit to
    /// list every cycle of the component.
    #[serde(default)]
    pub cycle: Option<String>,
    /// Case-insensitive substring filter applied to both the row topic
    /// (e.g. `"Cognitive Functioning"`) and the file stem (e.g. `"CFQ_J"`).
    /// Omit to keep every file.
    #[serde(default)]
    pub filter: Option<String>,
    /// Endpoint override for tests and mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Emit the NHANES component file listing as a table.
#[derive(Clone)]
pub struct NhanesFilesNode {
    meta: NodePorts,
    spec: NhanesFilesSpec,
}

pub struct NhanesFilesNodeFactory;

impl NodeFactory for NhanesFilesNodeFactory {
    fn kind(&self) -> &'static str {
        "source_nhanes_files"
    }

    fn desc(&self) -> &'static str {
        "List NHANES component data files as a table."
    }

    fn doc(&self) -> &'static str {
        "A zero-input source node for the CDC NHANES datapage listing. Output \
         schema: `cycle, topic, doc_href, href, file_stem, year, size_text`. \
         `cycle` and `topic` come from the listing row (all-cycle listings) \
         or the requested `cycle` (single-cycle listings omit the column), \
         never from the file name, so files with opaque stems (e.g. CFQ_F) \
         are findable by topic. `year` is the URL path year, which can differ \
         from the cycle start year. Feed `href` to source_nhanes_download to \
         fetch a file."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(NhanesFilesSpec)
    }

    fn ports(&self) -> NodePorts {
        listing_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(NhanesFilesNode {
            meta: listing_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for NhanesFilesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_nhanes_files"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let component = self.spec.component.trim();
        if component.is_empty() {
            return Err(DagError::Schedule(
                "source_nhanes_files: component cannot be empty; valid components: \
                 Demographics, Dietary, Examination, Laboratory, Questionnaire"
                    .into(),
            ));
        }
        let year = match self.spec.cycle.as_deref() {
            Some(cycle) => Some(cycle_begin_year(cycle.trim())?),
            None => None,
        };

        let html = client(self.spec.endpoint.as_deref())
            .listing_html(component, year)
            .await
            .map_err(|e| DagError::Schedule(format!("NHANES listing request failed: {e}")))?;
        if crate::is_soft_404(&html) {
            return Err(DagError::Schedule(format!(
                "source_nhanes_files: NHANES returned its \"Page Not Found\" template for \
                 component {component:?} (cycle {:?}); check the component spelling — valid \
                 components: {COMPONENTS}. Listing URL: \
                 https://wwwn.cdc.gov/nchs/nhanes/search/datapage.aspx?Component={component}&CycleBeginYear=<YYYY>",
                self.spec.cycle
            )));
        }
        let links = crate::parse_listing(&html);
        if links.is_empty() {
            return Err(DagError::Schedule(format!(
                "source_nhanes_files: no XPT file links found for component {component:?} \
                 (cycle {:?}); the component may not have files in that cycle. Inspect \
                 https://wwwn.cdc.gov/nchs/nhanes/search/datapage.aspx?Component={component} \
                 or omit `cycle` to list all cycles.",
                self.spec.cycle
            )));
        }

        let filter = self
            .spec
            .filter
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        let total = links.len();
        let selected: Vec<_> = match &filter {
            Some(needle) => links
                .into_iter()
                .filter(|link| {
                    link.topic.to_ascii_lowercase().contains(needle)
                        || link.file_stem.to_ascii_lowercase().contains(needle)
                })
                .collect(),
            None => links,
        };
        reporter.info(format!(
            "NHANES listing: {}/{} files for {component}",
            selected.len(),
            total
        ));

        let schema = Arc::new(Schema::new(vec![
            Field::new("cycle", DataType::Utf8, true),
            Field::new("topic", DataType::Utf8, false),
            Field::new("doc_href", DataType::Utf8, true),
            Field::new("href", DataType::Utf8, false),
            Field::new("file_stem", DataType::Utf8, false),
            Field::new("year", DataType::Utf8, true),
            Field::new("size_text", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                // All-cycle listings put the cycle in the row; single-cycle
                // listings omit the column, so the requested cycle backfills it.
                super::util::str_array(
                    selected
                        .iter()
                        .map(|l| l.cycle.clone().or_else(|| self.spec.cycle.clone()))
                        .collect(),
                ),
                super::util::str_array(selected.iter().map(|l| Some(l.topic.clone())).collect()),
                super::util::str_array(selected.iter().map(|l| l.doc_href.clone()).collect()),
                super::util::str_array(selected.iter().map(|l| Some(l.href.clone())).collect()),
                super::util::str_array(
                    selected.iter().map(|l| Some(l.file_stem.clone())).collect(),
                ),
                super::util::str_array(selected.iter().map(|l| l.year.clone()).collect()),
                super::util::str_array(selected.iter().map(|l| l.size_text.clone()).collect()),
            ],
        )
        .map_err(|e| DagError::Schedule(format!("failed to build NHANES listing batch: {e}")))?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read NHANES batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use arrow_array::{Array, StringArray};
    use dag_core::dag::node_event::NodeReporter;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_stub(responses: Vec<(&'static str, String)>) -> String {
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
                socket.write_all(body.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        endpoint
    }

    const LISTING_HTML: &str = r#"
<table>
<tr>
    <td>2017-2018</td>
    <td>Cognitive Functioning</td>
    <td><a href="/Nchs/Nhanes/2017/CFQ_J.htm">Doc</a></td>
    <td><a href="/Nchs/Data/Nhanes/Public/2017/DataFiles/CFQ_J.xpt">[XPT - 3.2 MB]</a></td>
    <td>Sept 2019</td>
</tr>
<tr>
    <td>1999-2000</td>
    <td>Homocysteine</td>
    <td><a href="https://wwwn.cdc.gov/Nchs/Nhanes/1999/L13.htm">Doc</a></td>
    <td><a href="https://wwwn.cdc.gov/Nchs/Data/Nhanes/Public/2001/DataFiles/L13.XPT">[XPT - 1.1 MB]</a></td>
    <td>Jan 2011</td>
</tr>
</table>
"#;

    /// Single-cycle listing (CycleBeginYear given): rows have no Years column.
    const SINGLE_CYCLE_HTML: &str = r#"
<table>
<tr>
    <td class="text-left">Demographic Variables and Sample Weights</td>
    <td class="text-center"><a href="/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.htm">DEMO_J Doc</a></td>
    <td class="text-center"><a href="/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.xpt">DEMO_J Data [XPT - 3.3 MB]</a></td>
    <td class="text-center">February 2020</td>
</tr>
<tr>
    <td class="text-left">Plasma Fasting Glucose</td>
    <td class="text-center"><a href="/Nchs/Data/Nhanes/Public/2017/DataFiles/GLU_J.htm">GLU_J Doc</a></td>
    <td class="text-center"><a href="/Nchs/Data/Nhanes/Public/2017/DataFiles/GLU_J.xpt">GLU_J Data [XPT - 1.2 MB]</a></td>
    <td class="text-center">October 2020</td>
</tr>
</table>
"#;

    async fn first_batch(mut node: Box<dyn DagNode>, ctx: &NodeCtx) -> RecordBatch {
        let outputs = node.execute(ctx, &[], &NodeReporter::noop()).await.unwrap();
        outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap()
            .remove(0)
    }

    fn utf8(batch: &RecordBatch, name: &str, row: usize) -> Option<String> {
        let column = batch.column_by_name(name).unwrap();
        let values = column.as_any().downcast_ref::<StringArray>().unwrap();
        if values.is_null(row) {
            None
        } else {
            Some(values.value(row).to_string())
        }
    }

    fn ctx() -> NodeCtx {
        NodeCtx::new(SessionContext::new().runtime_env(), None)
    }

    #[tokio::test]
    async fn lists_component_files_with_row_structure() {
        // No `cycle` in the spec: the all-cycles listing carries the Years
        // column and each row's own cycle label is emitted.
        let endpoint = spawn_stub(vec![("text/html", LISTING_HTML.to_string())]).await;
        let ctx = ctx();
        let spec = serde_json::json!({
            "component": "Questionnaire",
            "endpoint": endpoint
        });
        let node = NhanesFilesNodeFactory.build(spec, ctx.clone()).unwrap();
        let batch = first_batch(node, &ctx).await;
        assert_eq!(batch.num_columns(), 7);
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(utf8(&batch, "cycle", 0).unwrap(), "2017-2018");
        assert_eq!(utf8(&batch, "cycle", 1).unwrap(), "1999-2000");
        assert_eq!(utf8(&batch, "topic", 0).unwrap(), "Cognitive Functioning");
        assert_eq!(
            utf8(&batch, "href", 0).unwrap(),
            "/Nchs/Data/Nhanes/Public/2017/DataFiles/CFQ_J.xpt"
        );
        assert_eq!(utf8(&batch, "file_stem", 0).unwrap(), "CFQ_J");
        assert_eq!(utf8(&batch, "year", 1).unwrap(), "2001");
        assert_eq!(utf8(&batch, "size_text", 0).unwrap(), "[XPT - 3.2 MB]");
    }

    #[tokio::test]
    async fn single_cycle_listing_backfills_the_requested_cycle() {
        // A cycle-scoped request returns 4-column rows without a Years cell;
        // the cycle column is then filled from the requested `cycle`.
        let endpoint = spawn_stub(vec![("text/html", SINGLE_CYCLE_HTML.to_string())]).await;
        let ctx = ctx();
        let spec = serde_json::json!({
            "component": "Demographics",
            "cycle": "2017-2018",
            "endpoint": endpoint
        });
        let node = NhanesFilesNodeFactory.build(spec, ctx.clone()).unwrap();
        let batch = first_batch(node, &ctx).await;
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(utf8(&batch, "cycle", 0).unwrap(), "2017-2018");
        assert_eq!(utf8(&batch, "cycle", 1).unwrap(), "2017-2018");
        assert_eq!(
            utf8(&batch, "topic", 0).unwrap(),
            "Demographic Variables and Sample Weights"
        );
        assert_eq!(utf8(&batch, "file_stem", 0).unwrap(), "DEMO_J");
        assert_eq!(utf8(&batch, "file_stem", 1).unwrap(), "GLU_J");
        assert_eq!(utf8(&batch, "size_text", 0).unwrap(), "[XPT - 3.3 MB]");
    }

    #[tokio::test]
    async fn filter_matches_topic_and_file_stem_case_insensitively() {
        // Topic hit: "cognitive" matches the CFQ row; file-stem hit: "l13"
        // matches the Homocysteine row — both lowercase against mixed case.
        let endpoint = spawn_stub(vec![
            ("text/html", LISTING_HTML.to_string()),
            ("text/html", LISTING_HTML.to_string()),
        ])
        .await;
        let ctx = ctx();

        let spec = serde_json::json!({
            "component": "Questionnaire",
            "filter": "cognitive",
            "endpoint": endpoint
        });
        let node = NhanesFilesNodeFactory.build(spec, ctx.clone()).unwrap();
        let batch = first_batch(node, &ctx).await;
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(utf8(&batch, "file_stem", 0).unwrap(), "CFQ_J");

        let spec = serde_json::json!({
            "component": "Questionnaire",
            "filter": "l13",
            "endpoint": endpoint
        });
        let node = NhanesFilesNodeFactory.build(spec, ctx.clone()).unwrap();
        let batch = first_batch(node, &ctx).await;
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(utf8(&batch, "topic", 0).unwrap(), "Homocysteine");
    }

    #[tokio::test]
    async fn soft_404_listing_fails_with_guidance() {
        let endpoint = spawn_stub(vec![(
            "text/html",
            "<html><title>Page Not Found</title></html>".to_string(),
        )])
        .await;
        let ctx = ctx();
        let spec = serde_json::json!({
            "component": "Bogus",
            "cycle": "2017-2018",
            "endpoint": endpoint
        });
        let mut node = NhanesFilesNodeFactory.build(spec, ctx.clone()).unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Page Not Found"), "{message}");
        assert!(message.contains("Demographics"), "{message}");
        assert!(message.contains("datapage.aspx"), "{message}");
    }

    #[tokio::test]
    async fn empty_component_and_malformed_cycle_are_rejected() {
        let ctx = ctx();
        let mut node = NhanesFilesNodeFactory
            .build(
                serde_json::json!({"component": "  ", "endpoint": "http://127.0.0.1:1"}),
                ctx.clone(),
            )
            .unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("component cannot be empty"));

        let mut node = NhanesFilesNodeFactory
            .build(
                serde_json::json!({
                    "component": "Laboratory",
                    "cycle": "99-00",
                    "endpoint": "http://127.0.0.1:1"
                }),
                ctx.clone(),
            )
            .unwrap();
        let error = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("four-digit year"));
    }

    #[test]
    fn cycle_labels_resolve_to_start_years() {
        assert_eq!(cycle_begin_year("2017-2018").unwrap(), 2017);
        assert_eq!(cycle_begin_year("1999").unwrap(), 1999);
        assert_eq!(cycle_begin_year("1999-2000 ").unwrap(), 1999);
        assert!(cycle_begin_year("99-00").is_err());
    }

    /// Live check against CDC: the 2017-2018 Demographics listing must
    /// contain DEMO_J with its topic spelled out.
    #[tokio::test]
    #[ignore = "hits the live CDC NHANES endpoint"]
    async fn live_demographics_listing_contains_demo_j() {
        let ctx = ctx();
        let spec = serde_json::json!({
            "component": "Demographics",
            "cycle": "2017-2018"
        });
        let node = NhanesFilesNodeFactory.build(spec, ctx.clone()).unwrap();
        let batch = first_batch(node, &ctx).await;
        let values = batch
            .column_by_name("file_stem")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let stems: Vec<&str> = (0..values.len()).map(|i| values.value(i)).collect();
        assert!(stems.contains(&"DEMO_J"), "stems: {stems:?}");
    }
}
