//! DAG nodes over the local PubMed DuckDB mirror (feature `pubmed_local`).
//!
//! Three node kinds, all read-only against the mirror:
//!
//! - `source_pubmed_local` — structured query against the 40M-article
//!   mirror; emits an `evidence` artifact (small batches, joins the
//!   evidence pipeline) or a Parquet artifact (high-throughput, up to
//!   1M rows via DuckDB's native Parquet writer).
//! - `pubmed_mesh_analysis` — MeSH co-occurrence / year-trend aggregation
//!   over the 342M-row `article_mesh` table, emitted as Parquet.
//! - `pubmed_author_network` — co-author edge list from the authors column
//!   of a filtered article subset, emitted as Parquet.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use bib_types::evidence::{EvidenceRecord, EvidenceSet, FORMAT as EVIDENCE_FORMAT};
use bib_types::query::StructuredSearch;
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::nodes::write_artifact;
use crate::pubmed_local::{self, SharedLocalPubmed};
use crate::query::LiteratureSource;

/// Parquet format tag used on output ports and `FileRef`s.
const PARQUET_FORMAT: &str = "parquet";

/// Escape a string as a single-quoted SQL literal (mirror of the source
/// module's helper; duplicated so it stays crate-private there).
fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// ---------------------------------------------------------------------------
// Shared mirror handle
// ---------------------------------------------------------------------------

/// Process-wide mirror: one read-only DuckDB connection for every node.
static MIRROR: OnceLock<SharedLocalPubmed> = OnceLock::new();

fn shared_mirror() -> Result<SharedLocalPubmed, DagError> {
    if let Some(m) = MIRROR.get() {
        return Ok(m.clone());
    }
    let db = pubmed_local::try_open_from_env().ok_or_else(|| {
        DagError::Schedule(format!(
            "local pubmed mirror unavailable at {} (set {} or check for a writer lock; \
             see `pubmed_local` docs)",
            pubmed_local::db_path_from_env().display(),
            pubmed_local::DB_PATH_ENV,
        ))
    })?;
    let _ = MIRROR.set(db.clone());
    Ok(MIRROR.get().expect("just set").clone())
}

/// Read a DuckDB Parquet export from a temp file into a VFS artifact.
async fn export_artifact(
    node_ctx: &NodeCtx,
    tmp: &std::path::Path,
    dest: &str,
) -> Result<PortOutputs, DagError> {
    let bytes = tokio::fs::read(tmp)
        .await
        .map_err(|e| DagError::Schedule(format!("read parquet export {}: {e}", tmp.display())))?;
    let _ = tokio::fs::remove_file(tmp).await;
    let file = write_artifact(node_ctx, dest, bytes, PARQUET_FORMAT).await?;
    let mut outputs = PortOutputs::new();
    outputs.insert_file(0, file);
    Ok(outputs)
}

// ---------------------------------------------------------------------------
// source_pubmed_local
// ---------------------------------------------------------------------------

/// Default / hard cap on rows for the evidence output (small batches).
const DEFAULT_EVIDENCE_LIMIT: u64 = 100;
const MAX_EVIDENCE_LIMIT: u64 = 5_000;
/// Default rows for the Parquet output (high throughput).
const DEFAULT_PARQUET_LIMIT: u64 = 100_000;

/// Output shape for [`PubmedLocalNode`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PubmedLocalOutput {
    /// Deduplicated `evidence` JSON artifact — joins evidence_merge /
    /// evidence_export. Row-capped (default 100, max 5 000).
    #[default]
    Evidence,
    /// Parquet artifact with raw article columns — high-throughput path
    /// (default 100 000 rows, cap 1 000 000), written by DuckDB's native
    /// Parquet writer.
    Parquet,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct PubmedLocalSpec {
    /// Structured query against the local mirror. Keyword/title terms use
    /// the FTS index (BM25-ranked); MeSH / journal / author / year /
    /// publication-type filters are exact or indexed SQL.
    pub query: StructuredSearch,
    /// Output artifact shape (default `evidence`).
    #[serde(default)]
    pub output: PubmedLocalOutput,
    /// Max rows (evidence: default 100, cap 5 000; parquet: default
    /// 100 000, cap 1 000 000).
    #[serde(default)]
    pub limit: Option<u64>,
    /// Destination artifact path (`vfs://` URI or bare absolute path).
    pub path: String,
    /// Provenance note stamped on evidence records.
    #[serde(default)]
    pub note: Option<String>,
}

pub struct PubmedLocalNode {
    meta: NodePorts,
    spec: PubmedLocalSpec,
    /// Test seam: an injected mirror handle.
    mirror: Option<SharedLocalPubmed>,
}

impl PubmedLocalNode {
    pub fn new(spec: PubmedLocalSpec) -> Self {
        let meta = port_layout(spec.output);
        Self { meta, spec, mirror: None }
    }

    pub fn with_mirror(spec: PubmedLocalSpec, mirror: SharedLocalPubmed) -> Self {
        let meta = port_layout(spec.output);
        Self { meta, spec, mirror: Some(mirror) }
    }

    fn mirror(&self) -> Result<SharedLocalPubmed, DagError> {
        match &self.mirror {
            Some(m) => Ok(m.clone()),
            None => shared_mirror(),
        }
    }
}

fn port_layout(output: PubmedLocalOutput) -> NodePorts {
    match output {
        PubmedLocalOutput::Evidence => NodePorts::new()
            .add_output_port_of_type_with_label_and_format(
                None,
                PortType::File,
                "evidence",
                EVIDENCE_FORMAT,
            ),
        PubmedLocalOutput::Parquet => NodePorts::new()
            .add_output_port_of_type_with_label_and_format(
                None,
                PortType::File,
                "articles",
                PARQUET_FORMAT,
            ),
    }
}

#[async_trait]
impl DagNode for PubmedLocalNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            spec: self.spec.clone(),
            mirror: self.mirror.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        "source_pubmed_local"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let mirror = self.mirror()?;
        match self.spec.output {
            PubmedLocalOutput::Evidence => {
                let limit = self
                    .spec
                    .limit
                    .unwrap_or(DEFAULT_EVIDENCE_LIMIT)
                    .min(MAX_EVIDENCE_LIMIT) as usize;
                let source = pubmed_local::LocalPubmedSource::new(mirror);
                let batch = source
                    .search(&self.spec.query, limit)
                    .await
                    .map_err(|e| DagError::Schedule(format!("pubmed_local search: {e}")))?;
                let mut set = EvidenceSet::default();
                for citation in batch.articles {
                    set.records.push(EvidenceRecord {
                        citation,
                        note: self.spec.note.clone(),
                        origin: Some("pubmed_local".into()),
                    });
                }
                set.dedup_in_place();
                let bytes = set.to_bytes().map_err(DagError::Schedule)?;
                let file = write_artifact(node_ctx, &self.spec.path, bytes, EVIDENCE_FORMAT).await?;
                let mut outputs = PortOutputs::new();
                outputs.insert_file(0, file);
                Ok(outputs)
            }
            PubmedLocalOutput::Parquet => {
                let limit = self
                    .spec
                    .limit
                    .unwrap_or(DEFAULT_PARQUET_LIMIT)
                    .min(pubmed_local::MAX_EXPORT_ROWS);
                let select = pubmed_local::compile_search(
                    mirror.fts_available(),
                    &self.spec.query,
                    limit,
                )
                .map_err(|e| DagError::Schedule(format!("pubmed_local query: {e}")))?;
                let tmp = std::env::temp_dir()
                    .join(format!("pubmed-local-export-{}.parquet", uuid::Uuid::new_v4()));
                let copy_sql = format!(
                    "COPY ({select}) TO '{}' (FORMAT PARQUET)",
                    tmp.display()
                );
                mirror
                    .query_export(&copy_sql, &tmp)
                    .await
                    .map_err(|e| DagError::Schedule(format!("pubmed_local export: {e}")))?;
                export_artifact(node_ctx, &tmp, &self.spec.path).await
            }
        }
    }
}

pub struct PubmedLocalNodeFactory {}

#[async_trait]
impl NodeFactory for PubmedLocalNodeFactory {
    fn kind(&self) -> &'static str {
        "source_pubmed_local"
    }

    fn desc(&self) -> &'static str {
        "Query the local PubMed mirror (40M articles) and emit evidence or Parquet"
    }

    fn doc(&self) -> &'static str {
        "Queries the local PubMed DuckDB mirror (baseline + updatefiles, \
         ~40M citations, read-only). Two output shapes: `evidence` (default; \
         deduplicated JSON evidence artifact capped at 5 000 rows, consumable \
         by evidence_merge / evidence_export) and `parquet` (high-throughput; \
         DuckDB-native Parquet export of raw article columns capped at \
         1 000 000 rows). Keyword/title terms use the BM25 FTS index; \
         MeSH, journal, author, year and publication-type filters compile \
         to indexed SQL. No network access — the mirror lags live PubMed \
         by whatever updatefiles have been imported."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PubmedLocalSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout(PubmedLocalOutput::Evidence)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PubmedLocalSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PubmedLocalNode::new(spec)))
    }
}

// ---------------------------------------------------------------------------
// pubmed_mesh_analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct PubmedMeshSpec {
    /// Restrict the analysis to articles tagged with **any** of these MeSH
    /// descriptors (exact names, e.g. `"Telomere Shortening"`). Optional —
    /// omit for whole-corpus statistics (expensive: 342M rows).
    #[serde(default)]
    pub descriptors: Option<Vec<String>>,
    /// Restrict to a publication-year window.
    #[serde(default)]
    pub year_range: Option<bib_types::query::YearRange>,
    /// Analysis mode: `cooccurrence` (default) — descriptor pair counts
    /// within the selected article set; `trend` — per-descriptor,
    /// per-year article counts.
    #[serde(default)]
    pub mode: PubmedMeshMode,
    /// Minimum pair count (cooccurrence mode) or per-year count (trend
    /// mode) to emit. Default 5.
    #[serde(default = "default_min_count")]
    pub min_count: u64,
    /// Destination Parquet artifact path.
    pub path: String,
}

fn default_min_count() -> u64 {
    5
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PubmedMeshMode {
    #[default]
    Cooccurrence,
    Trend,
}

pub struct PubmedMeshNode {
    meta: NodePorts,
    spec: PubmedMeshSpec,
    mirror: Option<SharedLocalPubmed>,
}

impl PubmedMeshNode {
    pub fn new(spec: PubmedMeshSpec) -> Self {
        Self { meta: mesh_port_layout(), spec, mirror: None }
    }

    pub fn with_mirror(spec: PubmedMeshSpec, mirror: SharedLocalPubmed) -> Self {
        Self { meta: mesh_port_layout(), spec, mirror: Some(mirror) }
    }

    fn mirror(&self) -> Result<SharedLocalPubmed, DagError> {
        match &self.mirror {
            Some(m) => Ok(m.clone()),
            None => shared_mirror(),
        }
    }
}

fn mesh_port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type_with_label_and_format(
        None,
        PortType::File,
        "mesh",
        PARQUET_FORMAT,
    )
}

#[async_trait]
impl DagNode for PubmedMeshNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            spec: self.spec.clone(),
            mirror: self.mirror.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        "pubmed_mesh_analysis"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let mirror = self.mirror()?;

        // Article filter: descriptor membership + year window (literal SQL).
        let mut filter = String::from(
            "SELECT DISTINCT m.pmid FROM article_mesh m \
             JOIN articles a ON a.pmid = m.pmid WHERE TRUE",
        );
        if let Some(desc) = self.spec.descriptors.as_ref().filter(|d| !d.is_empty()) {
            let list = desc.iter().map(|d| sql_text(d)).collect::<Vec<_>>().join(",");
            filter.push_str(&format!(" AND m.descriptor_name IN ({list})"));
        }
        if let Some(range) = self.spec.year_range.as_ref() {
            filter.push_str(&format!(
                " AND try_cast(substr(a.pub_date, 1, 4) AS INTEGER) BETWEEN {} AND {}",
                range.from, range.to
            ));
        }
        let min = self.spec.min_count;

        let sql = match self.spec.mode {
            PubmedMeshMode::Cooccurrence => format!(
                "WITH selected AS ({filter}) \
                 SELECT m1.descriptor_name AS descriptor_a, m2.descriptor_name AS descriptor_b, \
                        count(*) AS pair_count \
                 FROM article_mesh m1 \
                 JOIN selected s ON s.pmid = m1.pmid \
                 JOIN article_mesh m2 ON m2.pmid = m1.pmid AND m2.descriptor_name > m1.descriptor_name \
                 GROUP BY 1, 2 HAVING count(*) >= {min} \
                 ORDER BY pair_count DESC"
            ),
            PubmedMeshMode::Trend => format!(
                "WITH selected AS ({filter}) \
                 SELECT m.descriptor_name, \
                        try_cast(substr(a.pub_date, 1, 4) AS INTEGER) AS year, \
                        count(*) AS article_count \
                 FROM article_mesh m \
                 JOIN selected s ON s.pmid = m.pmid \
                 JOIN articles a ON a.pmid = m.pmid \
                 GROUP BY 1, 2 HAVING count(*) >= {min} \
                 ORDER BY descriptor_name, year"
            ),
        };

        let tmp = std::env::temp_dir()
            .join(format!("pubmed-mesh-{}.parquet", uuid::Uuid::new_v4()));
        let copy_sql = format!("COPY ({sql}) TO '{}' (FORMAT PARQUET)", tmp.display());

        reporter.info(
            "pubmed_mesh_analysis: aggregating in DuckDB (whole-corpus queries take minutes)"
                .to_string(),
        );
        mirror
            .query_export(&copy_sql, &tmp)
            .await
            .map_err(|e| DagError::Schedule(format!("pubmed mesh export: {e}")))?;
        export_artifact(node_ctx, &tmp, &self.spec.path).await
    }
}

pub struct PubmedMeshNodeFactory {}

#[async_trait]
impl NodeFactory for PubmedMeshNodeFactory {
    fn kind(&self) -> &'static str {
        "pubmed_mesh_analysis"
    }

    fn desc(&self) -> &'static str {
        "MeSH co-occurrence or year-trend aggregation over the local PubMed mirror"
    }

    fn doc(&self) -> &'static str {
        "Aggregates the 342M-row `article_mesh` table of the local PubMed \
         mirror. Mode `cooccurrence` (default): counts descriptor pairs \
         co-annotated on the same articles — a topic landscape map. Mode \
         `trend`: per-descriptor, per-year article counts — topic \
         popularity over time. Articles can be restricted to a descriptor \
         subset and/or publication-year window; `min_count` prunes noise \
         (default 5). Output: Parquet artifact (descriptor_a/descriptor_b/\
         pair_count, or descriptor/year/article_count)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PubmedMeshSpec)
    }

    fn ports(&self) -> NodePorts {
        mesh_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PubmedMeshSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PubmedMeshNode::new(spec)))
    }
}

// ---------------------------------------------------------------------------
// pubmed_author_network
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct PubmedAuthorsSpec {
    /// Restrict to articles matching this structured query (author /
    /// journal / MeSH / year / publication-type filters; keyword terms
    /// require the FTS index). Keep it selective — MeSH descriptor + year
    /// window is the sweet spot.
    pub query: StructuredSearch,
    /// Minimum co-authored papers for an edge to be emitted. Default 2.
    #[serde(default = "default_min_pairs")]
    pub min_pairs: u64,
    /// Destination Parquet artifact path.
    pub path: String,
}

fn default_min_pairs() -> u64 {
    2
}

pub struct PubmedAuthorsNode {
    meta: NodePorts,
    spec: PubmedAuthorsSpec,
    mirror: Option<SharedLocalPubmed>,
}

impl PubmedAuthorsNode {
    pub fn new(spec: PubmedAuthorsSpec) -> Self {
        Self { meta: authors_port_layout(), spec, mirror: None }
    }

    pub fn with_mirror(spec: PubmedAuthorsSpec, mirror: SharedLocalPubmed) -> Self {
        Self { meta: authors_port_layout(), spec, mirror: Some(mirror) }
    }

    fn mirror(&self) -> Result<SharedLocalPubmed, DagError> {
        match &self.mirror {
            Some(m) => Ok(m.clone()),
            None => shared_mirror(),
        }
    }
}

fn authors_port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type_with_label_and_format(
        None,
        PortType::File,
        "edges",
        PARQUET_FORMAT,
    )
}

#[async_trait]
impl DagNode for PubmedAuthorsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            spec: self.spec.clone(),
            mirror: self.mirror.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        "pubmed_author_network"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let mirror = self.mirror()?;
        let inner = pubmed_local::compile_search(
            mirror.fts_available(),
            &self.spec.query,
            pubmed_local::MAX_EXPORT_ROWS,
        )
        .map_err(|e| DagError::Schedule(format!("pubmed_local query: {e}")))?;
        // The inner SELECT carries its own ORDER BY/LIMIT — irrelevant for
        // the pair aggregation; strip the tail by wrapping as a derived
        // table (DuckDB allows ORDER BY inside subqueries).
        let min = self.spec.min_pairs;
        let sql = format!(
            "WITH selected AS ({inner}), \
             names AS ( \
                 SELECT s.pmid, trim(unnest(string_split(a.authors, ';'))) AS author \
                 FROM selected s JOIN articles a ON a.pmid = s.pmid \
                 WHERE a.authors IS NOT NULL AND length(a.authors) > 0 \
             ), \
             pairs AS ( \
                 SELECT n1.author AS author_a, n2.author AS author_b, count(*) AS weight \
                 FROM names n1 JOIN names n2 ON n2.pmid = n1.pmid AND n2.author > n1.author \
                 GROUP BY 1, 2 HAVING count(*) >= {min} \
             ) \
             SELECT * FROM pairs ORDER BY weight DESC"
        );

        let tmp = std::env::temp_dir()
            .join(format!("pubmed-authors-{}.parquet", uuid::Uuid::new_v4()));
        let copy_sql = format!("COPY ({sql}) TO '{}' (FORMAT PARQUET)", tmp.display());
        mirror
            .query_export(&copy_sql, &tmp)
            .await
            .map_err(|e| DagError::Schedule(format!("pubmed author export: {e}")))?;
        export_artifact(node_ctx, &tmp, &self.spec.path).await
    }
}

pub struct PubmedAuthorsNodeFactory {}

#[async_trait]
impl NodeFactory for PubmedAuthorsNodeFactory {
    fn kind(&self) -> &'static str {
        "pubmed_author_network"
    }

    fn desc(&self) -> &'static str {
        "Co-author edge list from a filtered article subset of the local PubMed mirror"
    }

    fn doc(&self) -> &'static str {
        "Builds a co-authorship network from the local PubMed mirror. \
         `query` (a StructuredSearch) selects the article subset — keep it \
         selective (MeSH descriptor + year window is the sweet spot); \
         authors are split from the stored ';'-separated name lists and \
         every within-paper author pair becomes an edge, weighted by \
         co-authored paper count, pruned by `min_pairs` (default 2). \
         Output: Parquet (author_a, author_b, weight)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PubmedAuthorsSpec)
    }

    fn ports(&self) -> NodePorts {
        authors_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: PubmedAuthorsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PubmedAuthorsNode::new(spec)))
    }
}

// ---------------------------------------------------------------------------
// Tests (tiny fixture mirror; FTS paths need the real database)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dag_core::dag::node_event::NodeReporter;

    fn fixture_mirror() -> SharedLocalPubmed {
        let dir = std::env::temp_dir().join(format!("pubmed-nodes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pubmed.duckdb");
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE articles (pmid VARCHAR, doi VARCHAR, title VARCHAR, \
                abstract_text VARCHAR, authors VARCHAR, journal VARCHAR, issn VARCHAR, \
                volume VARCHAR, issue VARCHAR, pages VARCHAR, pub_date VARCHAR, \
                language VARCHAR, publication_types VARCHAR, publication_status VARCHAR); \
             INSERT INTO articles VALUES \
               ('1','10.1/a','CRISPR base editing review','A review of base editing.', \
                'Alice B; Carol D','Nature','1234-5678','21','2','1-9','2023-Jan','eng', \
                'Review,Journal Article','ppublish'), \
               ('2','10.1/b','Telomere shortening','Telomeres shorten.', \
                'Alice B; Eve F','Cell','0987-6543','5','1','e1','2022 Feb 3','eng', \
                'Journal Article','ppublish'); \
             CREATE TABLE article_mesh (pmid VARCHAR, ui VARCHAR, descriptor_name VARCHAR, \
                major_topic_yn VARCHAR); \
             INSERT INTO article_mesh VALUES ('1','D0001','CRISPR','Y'), \
                ('1','D0002','Genetic Editing','N'), \
                ('2','D0002','Telomere Shortening','N'); \
             CREATE TABLE article_keywords (pmid VARCHAR, keyword VARCHAR); \
             INSERT INTO article_keywords VALUES ('1','base editing');",
        )
        .unwrap();
        drop(conn);
        Arc::new(pubmed_local::LocalPubmedDb::open(&path).unwrap())
    }

    fn node_ctx() -> (NodeCtx, Arc<vfs::OpendalFileStorage>) {
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage.clone()),
        );
        (ctx, storage)
    }

    fn tmp_path(tag: &str, ext: &str) -> String {
        format!("/tmp/pubmed-node-{tag}-{}.{ext}", uuid::Uuid::new_v4())
    }

    async fn run(
        mut node: Box<dyn DagNode>,
        ctx: &NodeCtx,
    ) -> Result<PortOutputs, DagError> {
        let reporter = NodeReporter::noop();
        node.execute(ctx, &[], &reporter).await
    }

    #[tokio::test]
    async fn evidence_output_round_trips() {
        let (ctx, _storage) = node_ctx();
        let spec = PubmedLocalSpec {
            query: StructuredSearch {
                mesh: Some(vec!["Telomere Shortening".into()]),
                ..Default::default()
            },
            output: PubmedLocalOutput::Evidence,
            limit: None,
            path: tmp_path("evidence", "json"),
            note: Some("mirror test".into()),
        };
        let node = PubmedLocalNode::with_mirror(spec, fixture_mirror());
        let outputs = run(Box::new(node), &ctx).await.unwrap();
        let file = outputs.get(&0).unwrap().as_file().unwrap().clone();
        let bytes = crate::nodes::read_file_bytes(&ctx, &file.path).await.unwrap();
        let set = EvidenceSet::parse(&bytes).unwrap();
        assert_eq!(set.records.len(), 1);
        assert_eq!(set.records[0].origin.as_deref(), Some("pubmed_local"));
        assert_eq!(set.records[0].note.as_deref(), Some("mirror test"));
        assert_eq!(set.records[0].citation.title, "Telomere shortening");
    }

    #[tokio::test]
    async fn parquet_output_writes_duckdb_parquet() {
        let (ctx, _storage) = node_ctx();
        let spec = PubmedLocalSpec {
            query: StructuredSearch {
                year_range: Some(bib_types::query::YearRange { from: 2020, to: 2024 }),
                ..Default::default()
            },
            output: PubmedLocalOutput::Parquet,
            limit: None,
            path: tmp_path("parquet", "parquet"),
            note: None,
        };
        let node = PubmedLocalNode::with_mirror(spec, fixture_mirror());
        let outputs = run(Box::new(node), &ctx).await.unwrap();
        let file = outputs.get(&0).unwrap().as_file().unwrap().clone();
        assert_eq!(file.format.as_deref(), Some("parquet"));
        let bytes = crate::nodes::read_file_bytes(&ctx, &file.path).await.unwrap();
        // Parquet magic at file start and one-byte EOF marker at the end.
        assert_eq!(&bytes[0..4], b"PAR1");
        assert_eq!(&bytes[bytes.len() - 4..], b"PAR1");
    }

    #[tokio::test]
    async fn mesh_cooccurrence_counts_pairs() {
        let (ctx, _storage) = node_ctx();
        let spec = PubmedMeshSpec {
            descriptors: Some(vec!["CRISPR".into(), "Genetic Editing".into()]),
            year_range: None,
            mode: PubmedMeshMode::Cooccurrence,
            min_count: 1,
            path: tmp_path("mesh-pairs", "parquet"),
        };
        let node = PubmedMeshNode::with_mirror(spec, fixture_mirror());
        let outputs = run(Box::new(node), &ctx).await.unwrap();
        let file = outputs.get(&0).unwrap().as_file().unwrap().clone();
        let bytes = crate::nodes::read_file_bytes(&ctx, &file.path).await.unwrap();
        assert_eq!(&bytes[0..4], b"PAR1");
        // Decode with duckdb itself to assert the row count.
        let out = std::env::temp_dir().join(format!("mesh-check-{}.duckdb", uuid::Uuid::new_v4()));
        let tmp_write = std::env::temp_dir().join(format!("mesh-bytes-{}.parquet", uuid::Uuid::new_v4()));
        std::fs::write(&tmp_write, &bytes).unwrap();
        let conn = duckdb::Connection::open(&out).unwrap();
        let n: i64 = conn
            .query_row(
                &format!(
                    "SELECT count(*) FROM read_parquet('{}')",
                    tmp_write.display()
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "one co-annotated pair on article 1");
    }

    #[tokio::test]
    async fn author_network_emits_shared_edge() {
        let (ctx, _storage) = node_ctx();
        let spec = PubmedAuthorsSpec {
            query: StructuredSearch {
                journal: Some(vec!["Nature".into(), "Cell".into()]),
                ..Default::default()
            },
            min_pairs: 1,
            path: tmp_path("authors", "parquet"),
        };
        let node = PubmedAuthorsNode::with_mirror(spec, fixture_mirror());
        let outputs = run(Box::new(node), &ctx).await.unwrap();
        let file = outputs.get(&0).unwrap().as_file().unwrap().clone();
        let bytes = crate::nodes::read_file_bytes(&ctx, &file.path).await.unwrap();
        let tmp_write =
            std::env::temp_dir().join(format!("authors-bytes-{}.parquet", uuid::Uuid::new_v4()));
        std::fs::write(&tmp_write, &bytes).unwrap();
        let out = std::env::temp_dir().join(format!("authors-check-{}.duckdb", uuid::Uuid::new_v4()));
        let conn = duckdb::Connection::open(&out).unwrap();
        let rows: Vec<(String, String, i64)> = conn
            .prepare(&format!(
                "SELECT author_a, author_b, weight FROM read_parquet('{}') ORDER BY 1",
                tmp_write.display()
            ))
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        // Alice B co-occurs with Carol D (paper 1) and Eve F (paper 2);
        // Carol D + Eve F never share a paper. Tie order between the two
        // Alice B edges is unspecified, so compare as a set.
        assert_eq!(rows.len(), 2, "{rows:?}");
        let mut pairs: Vec<(String, String)> =
            rows.iter().map(|(a, b, _)| (a.clone(), b.clone())).collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("Alice B".to_string(), "Carol D".to_string()),
                ("Alice B".to_string(), "Eve F".to_string()),
            ]
        );
        assert!(rows.iter().all(|&(_, _, w)| w == 1));
    }
}
