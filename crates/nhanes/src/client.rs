//! HTTP client and HTML listing parser for the CDC NHANES file catalog.

use crate::error::{NhanesError, Result};

pub const DEFAULT_ENDPOINT: &str = "https://wwwn.cdc.gov";

const USER_AGENT: &str = "nhanes-rs-sdk/0.1 (+https://wwwn.cdc.gov/nchs/nhanes/)";
const XPORT_MAGIC: &[u8] = b"HEADER RECORD*******";

/// One downloadable XPT file row from a component listing page.
///
/// Two row shapes occur on `datapage.aspx`: all-cycle listings carry a
/// leading Years cell (`cycle, topic, Doc, Data, updated`), single-cycle
/// listings omit it (`topic, Doc, Data, updated`). Both are parsed
/// structurally — `topic` comes from a row cell, never from the anchor text,
/// so keyword search sees the human-readable topic even when the file stem
/// is opaque, and a topic can never pair with a neighboring row's file.
#[derive(Debug, Clone, PartialEq)]
pub struct NhanesFileLink {
    /// Survey cycle label from the row, e.g. `"2017-2018"` or `"2021-2023"`.
    /// Present in all-cycle listings; single-cycle listings omit the column
    /// and leave this `None` (the requesting node knows the cycle it asked
    /// for).
    pub cycle: Option<String>,
    /// Topic label from the row, e.g. `"Cognitive Functioning"`.
    pub topic: String,
    /// href of the documentation page anchor, when the row has one.
    pub doc_href: Option<String>,
    /// href of the XPT data file. Root-relative on live pages
    /// (`/Nchs/Data/Nhanes/Public/<year>/DataFiles/<FILE>.xpt`), sometimes
    /// absolute. Always lowercase-`.xpt`-suffixed in practice, matched
    /// case-insensitively.
    pub href: String,
    /// File name without extension, e.g. `"DEMO_J"`.
    pub file_stem: String,
    /// Four-digit year segment from the URL path (`Public/<year>/`), when
    /// present. NOTE: this is the file's URL year, which can differ from the
    /// cycle start year (e.g. FOLATE_E sits under `Public/2007/`).
    pub year: Option<String>,
    /// Size label scraped from the anchor text, e.g. `"[XPT - 3.2 MB]"`.
    pub size_text: Option<String>,
}

/// Async client for the public CDC NHANES listing and download endpoints.
///
/// No API key is required. The base endpoint can be overridden with the
/// `ENDPOINT_NHANES_URL` environment variable or [`Self::with_endpoint`]
/// (used by tests to point at a local stub).
#[derive(Debug, Clone)]
pub struct NhanesClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for NhanesClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NhanesClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("reqwest client builder");
        Self {
            http,
            endpoint: std::env::var("ENDPOINT_NHANES_URL")
                .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string()),
        }
    }

    /// Point the client at a different base endpoint (test injection).
    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("reqwest client builder"),
            endpoint: endpoint.into(),
        }
    }

    /// Fetch the HTML listing page of one component.
    ///
    /// `cycle_begin_year` restricts the listing to one cycle (its start year,
    /// e.g. `Some(2017)` for 2017-2018); `None` returns every cycle of the
    /// component. Invalid component/year combinations do NOT return an HTTP
    /// error — CDC serves an HTTP 200 "Page Not Found" template. Check the
    /// body with [`is_soft_404`] / [`parse_listing`] before using the result.
    pub async fn listing_html(
        &self,
        component: &str,
        cycle_begin_year: Option<u16>,
    ) -> Result<String> {
        let mut request = self
            .http
            .get(format!(
                "{}/nchs/nhanes/search/datapage.aspx",
                self.endpoint
            ))
            .query(&[("Component", component)]);
        if let Some(year) = cycle_begin_year {
            request = request.query(&[("CycleBeginYear", &year.to_string())]);
        }
        Ok(request.send().await?.text().await?)
    }

    /// Download an XPT file and validate that it really is one.
    ///
    /// An HTML error page comes back with HTTP 200, so success is verified by
    /// content-type (must not be `text/html`) and the XPORT magic prefix
    /// (`HEADER RECORD*******`); anything else fails with
    /// [`NhanesError::InvalidDownload`] carrying status, content-type, and the
    /// first bytes of the body for diagnosis.
    pub async fn download_xpt(&self, href: &str) -> Result<Vec<u8>> {
        let url = absolute_url(&self.endpoint, href);
        let response = self.http.get(&url).send().await?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let bytes = response.bytes().await?;
        if !status.is_success()
            || content_type.contains("text/html")
            || !bytes.starts_with(XPORT_MAGIC)
        {
            let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).into_owned();
            return Err(NhanesError::InvalidDownload {
                url,
                status: status.as_u16(),
                content_type,
                prefix,
            });
        }
        Ok(bytes.to_vec())
    }
}

/// Detect the HTTP 200 "Page Not Found" template CDC serves for bad
/// component/year combinations.
pub fn is_soft_404(html: &str) -> bool {
    html.to_ascii_lowercase().contains("page not found")
}

/// Parse a `datapage.aspx` listing into one [`NhanesFileLink`] per table row
/// that has an XPT data-file anchor.
///
/// Rows are parsed structurally — the XPT anchor is located first and the
/// Doc anchor / topic / (when present) cycle label are read from the
/// preceding cells — rather than by scanning anchors flat across the page,
/// so a topic can never be paired with a neighboring row's file.
pub fn parse_listing(html: &str) -> Vec<NhanesFileLink> {
    let lower = html.to_ascii_lowercase();
    let mut links = Vec::new();
    let mut search_from = 0;
    while let Some(offset) = lower[search_from..].find("<tr") {
        let row_open = search_from + offset;
        let Some(tag_end) = lower[row_open..].find('>') else {
            break;
        };
        let row_start = row_open + tag_end + 1;
        let Some(close) = lower[row_start..].find("</tr") else {
            break;
        };
        let row_end = row_start + close;
        if let Some(link) = parse_row(&html[row_start..row_end]) {
            links.push(link);
        }
        search_from = row_end;
    }
    links
}

// ---------------------------------------------------------------------------
// Row parsing helpers (hand-rolled; no HTML crate in the dependency set)
// ---------------------------------------------------------------------------

struct AnchorRef {
    href: String,
    text: String,
}

fn parse_row(row: &str) -> Option<NhanesFileLink> {
    let cells = td_cells(row);
    // Locate the XPT data anchor's cell; the Doc anchor and the topic sit in
    // the cells right before it, and the cycle label one cell before the
    // topic in all-cycle listings (single-cycle listings have no such cell).
    let data_cell = cells
        .iter()
        .position(|cell| anchors(cell).iter().any(|a| is_xpt_href(&a.href)))?;
    if data_cell < 2 {
        return None;
    }
    let data_anchor = anchors(&cells[data_cell])
        .into_iter()
        .find(|a| is_xpt_href(&a.href))?;
    let doc_href = anchors(&cells[data_cell - 1])
        .into_iter()
        .find(|a| is_doc_href(&a.href))
        .map(|a| a.href);
    let topic = plain_text(&cells[data_cell - 2]);
    if topic.is_empty() {
        return None;
    }
    let cycle = (data_cell >= 3)
        .then(|| plain_text(&cells[data_cell - 3]))
        .and_then(|cycle| (!cycle.is_empty()).then_some(cycle));
    let file_stem = file_stem(&data_anchor.href)?;
    let year = url_year(&data_anchor.href);
    Some(NhanesFileLink {
        cycle,
        topic,
        doc_href,
        href: data_anchor.href,
        file_stem,
        year,
        size_text: bracket_label(&data_anchor.text),
    })
}

/// Inner HTML of every `<td>` cell in a table row.
fn td_cells(row: &str) -> Vec<String> {
    let lower = row.to_ascii_lowercase();
    let mut cells = Vec::new();
    let mut search_from = 0;
    while let Some(offset) = lower[search_from..].find("<td") {
        let open = search_from + offset;
        let Some(tag_end) = lower[open..].find('>') else {
            break;
        };
        let content_start = open + tag_end + 1;
        let Some(close) = lower[content_start..].find("</td") else {
            break;
        };
        cells.push(row[content_start..content_start + close].to_string());
        search_from = content_start + close;
    }
    cells
}

/// All `<a href="…">text</a>` anchors inside a fragment.
fn anchors(fragment: &str) -> Vec<AnchorRef> {
    let lower = fragment.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(offset) = lower[search_from..].find("<a ") {
        let tag_start = search_from + offset;
        let Some(tag_end) = lower[tag_start..].find('>') else {
            break;
        };
        let tag_close = tag_start + tag_end;
        let Some(close) = lower[tag_close..].find("</a") else {
            break;
        };
        let text_end = tag_close + close;
        if let Some(href) = attr_value(&fragment[tag_start..tag_close], "href") {
            out.push(AnchorRef {
                href,
                text: plain_text(&fragment[tag_close + 1..text_end]),
            });
        }
        search_from = text_end + 1;
    }
    out
}

/// Value of `name="…"` (single or double quotes) inside a tag, entities
/// unescaped.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{name}=");
    let start = lower.find(&needle)? + needle.len();
    let quote = lower[start..].chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value_start = start + 1;
    let end = lower[value_start..].find(quote)?;
    Some(unescape(&tag[value_start..value_start + end]))
}

fn is_xpt_href(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    lower.contains("/nchs/data/nhanes/public/") && lower.ends_with(".xpt")
}

fn is_doc_href(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    lower.ends_with(".htm") || lower.ends_with(".html") || lower.ends_with(".aspx")
}

/// File name of an href (URL or path) minus its `.xpt` extension,
/// case-insensitively.
fn file_stem(href: &str) -> Option<String> {
    let name = href.rsplit('/').next()?;
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".xpt")?;
    Some(name[..stem.len()].to_string())
}

/// Four-digit year path segment after `Public/`, when present.
fn url_year(href: &str) -> Option<String> {
    let lower = href.to_ascii_lowercase();
    let idx = lower.find("/public/")? + "/public/".len();
    let rest = &href[idx..];
    let segment = rest.split('/').next()?;
    let digits: String = segment.chars().take_while(|c| c.is_ascii_digit()).collect();
    (digits.len() == 4).then_some(digits)
}

/// First `[…]` label in an anchor text, e.g. `"[XPT - 3.2 MB]"`.
fn bracket_label(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let end = text[start..].find(']')? + start;
    Some(text[start..=end].to_string())
}

/// Strip tags, unescape entities, collapse whitespace.
fn plain_text(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for c in unescape(fragment).chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Resolve a root-relative CDC href against the endpoint base; absolute
/// hrefs pass through untouched.
fn absolute_url(endpoint: &str, href: &str) -> String {
    let trimmed = href.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        let path = trimmed.trim_start_matches('/');
        format!("{endpoint}/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All-cycles view: every row carries a leading Years cell (5 columns).
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
<tr>
    <td>2001-2002</td>
    <td>Dioxins, Furans &amp; PCBs</td>
    <td><a href="/Nchs/Nhanes/2001/L28POC_B.htm">Doc</a></td>
    <td><a href="/Nchs/Data/Nhanes/Public/2001/DataFiles/L28POC_B.xpt">[XPT - 16.9 MB]</a></td>
    <td>Oct 2007</td>
</tr>
<tr>
    <td>2017-2018</td>
    <td>Doc-Only Row</td>
    <td><a href="/Nchs/Nhanes/2017/XXX.htm">Doc</a></td>
    <td><a href="/Nchs/Nhanes/2017/XXX.htm">[Documentation]</a></td>
    <td>Sept 2019</td>
</tr>
</table>
"#;

    /// Single-cycle view (CycleBeginYear given): the Years column is absent
    /// and rows have 4 columns (topic, Doc, Data, date published).
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

    #[test]
    fn parse_listing_extracts_row_structure() {
        let links = parse_listing(LISTING_HTML);
        assert_eq!(links.len(), 3, "doc-only row must be excluded");

        // Row 1: lowercase relative href, cycle/topic from cells, size label.
        let cfq = &links[0];
        assert_eq!(cfq.cycle.as_deref(), Some("2017-2018"));
        assert_eq!(cfq.topic, "Cognitive Functioning");
        assert_eq!(cfq.doc_href.as_deref(), Some("/Nchs/Nhanes/2017/CFQ_J.htm"));
        assert_eq!(
            cfq.href,
            "/Nchs/Data/Nhanes/Public/2017/DataFiles/CFQ_J.xpt"
        );
        assert_eq!(cfq.file_stem, "CFQ_J");
        assert_eq!(cfq.year.as_deref(), Some("2017"));
        assert_eq!(cfq.size_text.as_deref(), Some("[XPT - 3.2 MB]"));

        // Row 2: absolute URL with UPPERCASE .XPT; URL year (2001) differs
        // from the cycle start (1999) — the documented trap.
        let l13 = &links[1];
        assert_eq!(
            l13.href,
            "https://wwwn.cdc.gov/Nchs/Data/Nhanes/Public/2001/DataFiles/L13.XPT"
        );
        assert_eq!(l13.file_stem, "L13");
        assert_eq!(l13.year.as_deref(), Some("2001"));
        assert_eq!(l13.cycle.as_deref(), Some("1999-2000"));

        // Row 3: entities in the topic are unescaped.
        assert_eq!(links[2].topic, "Dioxins, Furans & PCBs");
    }

    #[test]
    fn parse_listing_handles_single_cycle_rows_without_a_years_column() {
        let links = parse_listing(SINGLE_CYCLE_HTML);
        assert_eq!(links.len(), 2);

        let demo = &links[0];
        assert_eq!(demo.cycle, None, "single-cycle rows carry no Years cell");
        assert_eq!(demo.topic, "Demographic Variables and Sample Weights");
        assert_eq!(
            demo.doc_href.as_deref(),
            Some("/Nchs/Data/Nhanes/Public/2017/DataFiles/DEMO_J.htm")
        );
        assert_eq!(demo.file_stem, "DEMO_J");
        assert_eq!(demo.year.as_deref(), Some("2017"));
        // Size label sits at the END of the anchor text, after "DEMO_J Data".
        assert_eq!(demo.size_text.as_deref(), Some("[XPT - 3.3 MB]"));
        assert_eq!(links[1].file_stem, "GLU_J");
    }

    #[test]
    fn soft_404_template_is_detected() {
        assert!(is_soft_404(
            "<html><head><title>Page Not Found</title></head></html>"
        ));
        assert!(!is_soft_404(LISTING_HTML));
        assert!(parse_listing("<html><body><h1>Page Not Found</h1></body></html>").is_empty());
    }

    #[test]
    fn absolute_url_joins_root_relative_hrefs() {
        assert_eq!(
            absolute_url("http://127.0.0.1:1", "/Nchs/Data/x.xpt"),
            "http://127.0.0.1:1/Nchs/Data/x.xpt"
        );
        assert_eq!(
            absolute_url("http://127.0.0.1:1", "https://other/x.XPT"),
            "https://other/x.XPT"
        );
    }
}
