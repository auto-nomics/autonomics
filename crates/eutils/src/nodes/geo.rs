//! GEO query endpoints as zero-input, one-DataFrame-output DAG source nodes.
//!
//! These nodes wrap the query and metadata E-utilities used with GEO. Bulk
//! record and supplementary-file transfer remains deliberately outside this
//! node family; use file download/VFS nodes for those workflows.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use crate::EutilsClient;
use crate::geo::{GEO_DATASETS_DB, GeoEntryType, GeoSearchRequest, GeoSummary};
use crate::nodes::util::{
    dataframe_output, dataframe_port, json_string, parse_u64, request_error, schedule_error,
    str_array, u64_array, uint64, utf8, value_string,
};
use crate::types::{EGQueryResponse, EInfoResponse, ELinkRequest, ESearchResponse};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_geo_node_factory {
    ($factory:ident, $node:ident, $spec:ident, $kind:literal, $description:literal, $documentation:literal) => {
        impl NodeFactory for $factory {
            fn kind(&self) -> &'static str {
                $kind
            }

            fn desc(&self) -> &'static str {
                $description
            }

            fn doc(&self) -> &'static str {
                $documentation
            }

            fn spec_schema(&self) -> schemars::Schema {
                schema_for!($spec)
            }

            fn ports(&self) -> NodePorts {
                dataframe_port()
            }

            fn build(
                &self,
                spec: serde_json::Value,
                _node_ctx: NodeCtx,
            ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
                let spec = serde_json::from_value::<$spec>(spec)?;
                Ok(Box::new($node {
                    meta: dataframe_port(),
                    spec,
                }))
            }
        }
    };
}

// ===========================================================================
// EInfo
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct GeoEInfoSpec {
    /// Entrez database name. Omit to list every Entrez database.
    #[serde(default)]
    pub database: Option<String>,
}

#[derive(Clone)]
pub struct GeoEInfoNode {
    meta: NodePorts,
    spec: GeoEInfoSpec,
}

pub struct GeoEInfoNodeFactory;

impl_geo_node_factory!(
    GeoEInfoNodeFactory,
    GeoEInfoNode,
    GeoEInfoSpec,
    "source_geo_einfo",
    "Returns GEO/Entrez database fields, counts, and links as a table.",
    "Wraps the E-utilities EInfo endpoint. Set database to `gds` for GEO \
     DataSets, `geoprofiles` for GEO Profiles, or omit it to list all Entrez \
     databases. Output columns include database metadata, searchable field \
     metadata, and a JSON aggregate of database links."
);

#[async_trait]
impl DagNode for GeoEInfoNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_geo_einfo"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let database = self
            .spec
            .database
            .as_deref()
            .map(str::trim)
            .filter(|database| !database.is_empty());
        let response = EutilsClient::from_env()
            .einfo(database)
            .await
            .map_err(|error| request_error(error, "source_geo_einfo"))?;
        let batch = build_einfo_batch(&response)?;
        dataframe_output(ctx, batch, "source_geo_einfo")
    }
}

struct EInfoRow {
    database: String,
    menu_name: Option<String>,
    description: Option<String>,
    record_count: Option<u64>,
    last_update: Option<String>,
    build: Option<String>,
    field_name: Option<String>,
    field_full_name: Option<String>,
    field_description: Option<String>,
    field_term_count: Option<u64>,
    links_json: Option<String>,
}

fn build_einfo_batch(response: &EInfoResponse) -> Result<RecordBatch, DagError> {
    let mut rows = Vec::new();
    if let Some(databases) = &response.result.dblist {
        rows.extend(databases.iter().map(|database| EInfoRow {
            database: database.clone(),
            menu_name: None,
            description: None,
            record_count: None,
            last_update: None,
            build: None,
            field_name: None,
            field_full_name: None,
            field_description: None,
            field_term_count: None,
            links_json: None,
        }));
    } else if let Some(databases) = &response.result.dbinfo {
        for database in databases {
            let links_json = database
                .linklist
                .as_ref()
                .and_then(|links| serde_json::to_value(links).ok())
                .as_ref()
                .and_then(json_string);
            let base = || EInfoRow {
                database: database.dbname.clone(),
                menu_name: database.menuname.clone(),
                description: database.description.clone(),
                record_count: database
                    .count
                    .as_deref()
                    .and_then(|count| count.parse::<u64>().ok()),
                last_update: database.lastupdate.clone(),
                build: database.dbbuild.clone(),
                field_name: None,
                field_full_name: None,
                field_description: None,
                field_term_count: None,
                links_json: links_json.clone(),
            };

            let fields = database.fieldlist.as_deref().unwrap_or(&[]);
            if fields.is_empty() {
                rows.push(base());
            } else {
                for field in fields {
                    let mut row = base();
                    row.field_name = field.get("name").and_then(value_string);
                    row.field_full_name = field.get("fullname").and_then(value_string);
                    row.field_description = field.get("description").and_then(value_string);
                    row.field_term_count = field.get("termcount").and_then(parse_u64);
                    rows.push(row);
                }
            }
        }
    }

    let schema = Arc::new(Schema::new(vec![
        utf8("database"),
        utf8("menu_name"),
        utf8("description"),
        uint64("record_count"),
        utf8("last_update"),
        utf8("build"),
        utf8("field_name"),
        utf8("field_full_name"),
        utf8("field_description"),
        uint64("field_term_count"),
        utf8("links_json"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.database.clone())).collect()),
            str_array(rows.iter().map(|row| row.menu_name.clone()).collect()),
            str_array(rows.iter().map(|row| row.description.clone()).collect()),
            u64_array(rows.iter().map(|row| row.record_count).collect()),
            str_array(rows.iter().map(|row| row.last_update.clone()).collect()),
            str_array(rows.iter().map(|row| row.build.clone()).collect()),
            str_array(rows.iter().map(|row| row.field_name.clone()).collect()),
            str_array(rows.iter().map(|row| row.field_full_name.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.field_description.clone())
                    .collect(),
            ),
            u64_array(rows.iter().map(|row| row.field_term_count).collect()),
            str_array(rows.iter().map(|row| row.links_json.clone()).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_geo_einfo", error))
}

// ===========================================================================
// ESearch
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct GeoESearchSpec {
    /// Raw GEO DataSets query. Useful for expert Entrez syntax.
    #[serde(default)]
    pub query: Option<String>,
    /// Exact GEO accession, such as `GSE48094`.
    #[serde(default)]
    pub accession: Option<String>,
    /// Organism filter.
    #[serde(default)]
    pub organism: Option<String>,
    /// GEO entry type: `GSE`, `GDS`, `GSM`, or `GPL`.
    #[serde(default)]
    pub entry_type: Option<String>,
    /// GEO dataset type filter.
    #[serde(default)]
    pub dataset_type: Option<String>,
    /// Page size.
    #[serde(default)]
    pub retmax: Option<u32>,
    /// Page start offset.
    #[serde(default)]
    pub retstart: Option<u32>,
    /// Entrez sort expression.
    #[serde(default)]
    pub sort: Option<String>,
    /// Store the result in the Entrez History server.
    #[serde(default)]
    pub usehistory: Option<bool>,
}

#[derive(Clone)]
pub struct GeoESearchNode {
    meta: NodePorts,
    spec: GeoESearchSpec,
}

pub struct GeoESearchNodeFactory;

impl_geo_node_factory!(
    GeoESearchNodeFactory,
    GeoESearchNode,
    GeoESearchSpec,
    "source_geo_esearch",
    "Runs a GEO DataSets ESearch and returns matching UIDs.",
    "Wraps the E-utilities ESearch endpoint against GEO DataSets (`gds`). \
     Typed filters are AND-ed with the optional raw query. Output columns \
     are uid, total_count, returned_count, start_offset, query_translation, \
     web_env, and query_key."
);

#[async_trait]
impl DagNode for GeoESearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_geo_esearch"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let request = geo_search_request(&self.spec)?;
        let response = EutilsClient::from_env()
            .geo_search(&request)
            .await
            .map_err(|error| request_error(error, "source_geo_esearch"))?;
        let batch = build_esearch_batch(&response)?;
        dataframe_output(ctx, batch, "source_geo_esearch")
    }
}

fn geo_search_request(spec: &GeoESearchSpec) -> Result<GeoSearchRequest, DagError> {
    let query = nonempty(&spec.query);
    let accession = nonempty(&spec.accession);
    let mut request = if let Some(accession) = accession {
        GeoSearchRequest::accession(accession)
            .map_err(|error| schedule_error("source_geo_esearch", error))?
    } else if let Some(query) = query {
        GeoSearchRequest::new(query)
    } else {
        GeoSearchRequest::default()
    };
    if let Some(query) = query {
        request.term = Some(query.to_owned());
    }
    if let Some(organism) = nonempty(&spec.organism) {
        request = request.with_organism(organism);
    }
    if let Some(entry_type) = nonempty(&spec.entry_type) {
        request = request.with_entry_type(parse_entry_type(entry_type)?);
    }
    if let Some(dataset_type) = nonempty(&spec.dataset_type) {
        request = request.with_dataset_type(dataset_type);
    }
    if let Some(retmax) = spec.retmax {
        request = request.with_retmax(retmax);
    }
    if let Some(retstart) = spec.retstart {
        request = request.with_retstart(retstart);
    }
    if let Some(sort) = nonempty(&spec.sort) {
        request = request.with_sort(sort);
    }
    if spec.usehistory.unwrap_or(false) {
        request = request.with_history();
    }
    Ok(request)
}

fn parse_entry_type(value: &str) -> Result<GeoEntryType, DagError> {
    match value.trim().to_ascii_uppercase().as_str() {
        "GSE" => Ok(GeoEntryType::Series),
        "GDS" => Ok(GeoEntryType::DataSet),
        "GSM" => Ok(GeoEntryType::Sample),
        "GPL" => Ok(GeoEntryType::Platform),
        other => Err(schedule_error(
            "source_geo_esearch",
            format!("unknown entry_type '{other}'; expected GSE, GDS, GSM, or GPL"),
        )),
    }
}

fn build_esearch_batch(response: &ESearchResponse) -> Result<RecordBatch, DagError> {
    let result = &response.result;
    let ids: Vec<Option<String>> = if result.id_list.is_empty() {
        vec![None]
    } else {
        result.id_list.iter().map(|id| Some(id.clone())).collect()
    };
    let count = ids.len();
    let schema = Arc::new(Schema::new(vec![
        utf8("uid"),
        uint64("total_count"),
        uint64("returned_count"),
        uint64("start_offset"),
        utf8("query_translation"),
        utf8("web_env"),
        utf8("query_key"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(ids),
            u64_array(vec![parse_u64(&Value::String(result.count.clone())); count]),
            u64_array(vec![
                parse_u64(&Value::String(result.retmax.clone()));
                count
            ]),
            u64_array(vec![
                parse_u64(&Value::String(result.retstart.clone()));
                count
            ]),
            str_array(vec![result.query_translation.clone(); count]),
            str_array(vec![result.web_env.clone(); count]),
            str_array(vec![result.query_key.clone(); count]),
        ],
    )
    .map_err(|error| schedule_error("source_geo_esearch", error))
}

// ===========================================================================
// ESummary
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct GeoESummarySpec {
    /// GEO DataSets Entrez UIDs, e.g. `["200048094"]`.
    pub uids: Vec<String>,
}

#[derive(Clone)]
pub struct GeoESummaryNode {
    meta: NodePorts,
    spec: GeoESummarySpec,
}

pub struct GeoESummaryNodeFactory;

impl_geo_node_factory!(
    GeoESummaryNodeFactory,
    GeoESummaryNode,
    GeoESummarySpec,
    "source_geo_esummary",
    "Fetches typed GEO DataSets summaries by Entrez UID.",
    "Wraps the E-utilities ESummary endpoint for GEO DataSets. UIDs come \
     from source_geo_esearch. Output includes accession, title, organism, \
     study type, sample accessions/titles, PubMed IDs, BioProject, and FTP \
     metadata. No supplementary files are downloaded."
);

#[async_trait]
impl DagNode for GeoESummaryNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_geo_esummary"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.spec.uids.iter().all(|uid| uid.trim().is_empty()) {
            return Err(schedule_error(
                "source_geo_esummary",
                "at least one non-empty GEO UID is required",
            ));
        }
        let records = EutilsClient::from_env()
            .geo_summaries(self.spec.uids.iter().map(String::as_str))
            .await
            .map_err(|error| request_error(error, "source_geo_esummary"))?;
        let batch = build_summary_batch(&records)?;
        dataframe_output(ctx, batch, "source_geo_esummary")
    }
}

fn build_summary_batch(records: &[GeoSummary]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("uid"),
        utf8("accession"),
        utf8("entry_type"),
        utf8("title"),
        utf8("summary"),
        utf8("taxon"),
        utf8("dataset_type"),
        utf8("platform_technology_type"),
        utf8("value_type"),
        utf8("platform_id"),
        utf8("series_id"),
        utf8("publication_date"),
        utf8("supplementary_files"),
        uint64("sample_count"),
        utf8("samples"),
        utf8("pubmed_ids"),
        utf8("projects"),
        utf8("ftp_link"),
        utf8("geo2r"),
        utf8("bioproject"),
        utf8("relations_json"),
        utf8("external_relations_json"),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            str_array(records.iter().map(|row| Some(row.uid.clone())).collect()),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.accession.clone()))
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.entry_type.clone()))
                    .collect(),
            ),
            str_array(records.iter().map(|row| Some(row.title.clone())).collect()),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.summary.clone()))
                    .collect(),
            ),
            str_array(records.iter().map(|row| Some(row.taxon.clone())).collect()),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.dataset_type.clone()))
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.platform_technology_type.clone()))
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.value_type.clone()))
                    .collect(),
            ),
            str_array(records.iter().map(|row| Some(row.gpl.clone())).collect()),
            str_array(records.iter().map(|row| Some(row.gse.clone())).collect()),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.publication_date.clone()))
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.supplementary_files.clone()))
                    .collect(),
            ),
            u64_array(records.iter().map(|row| row.n_samples).collect()),
            str_array(
                records
                    .iter()
                    .map(|row| {
                        (!row.samples.is_empty()).then(|| {
                            row.samples
                                .iter()
                                .map(|sample| format!("{}: {}", sample.accession, sample.title))
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                    })
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| (!row.pubmedids.is_empty()).then(|| row.pubmedids.join("; ")))
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| (!row.projects.is_empty()).then(|| row.projects.join("; ")))
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.ftp_link.clone()))
                    .collect(),
            ),
            str_array(records.iter().map(|row| Some(row.geo2r.clone())).collect()),
            str_array(
                records
                    .iter()
                    .map(|row| Some(row.bioproject.clone()))
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| serde_json::to_value(&row.relations).ok())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(|value| value.as_ref().and_then(json_string))
                    .collect(),
            ),
            str_array(
                records
                    .iter()
                    .map(|row| serde_json::to_value(&row.extrelations).ok())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(|value| value.as_ref().and_then(json_string))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_geo_esummary", error))
}

// ===========================================================================
// ELink
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct GeoELinkSpec {
    /// GEO DataSets source UIDs.
    pub uids: Vec<String>,
    /// Destination Entrez database, such as `pubmed`, `sra`, or `bioproject`.
    #[serde(default)]
    pub database: Option<String>,
    /// Restrict to a link name such as `gds_pubmed`.
    #[serde(default)]
    pub linkname: Option<String>,
    /// ELink command, such as `neighbor` or `neighbor_score`.
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Clone)]
pub struct GeoELinkNode {
    meta: NodePorts,
    spec: GeoELinkSpec,
}

pub struct GeoELinkNodeFactory;

impl_geo_node_factory!(
    GeoELinkNodeFactory,
    GeoELinkNode,
    GeoELinkSpec,
    "source_geo_elink",
    "Returns Entrez records linked to GEO DataSets UIDs.",
    "Wraps the E-utilities ELink endpoint with `dbfrom=gds`. Common GEO \
     destinations include pubmed, sra, bioproject, biosample, and taxonomy. \
     Output columns are source_uid, source_database, destination_database, \
     linkname, destination_uid, and score/rank when supplied by NCBI."
);

#[async_trait]
impl DagNode for GeoELinkNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_geo_elink"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.spec.uids.iter().all(|uid| uid.trim().is_empty()) {
            return Err(schedule_error(
                "source_geo_elink",
                "at least one non-empty GEO UID is required",
            ));
        }
        let mut request = ELinkRequest::new(GEO_DATASETS_DB, &self.spec.uids.join(","));
        request.db = nonempty(&self.spec.database).map(str::to_owned);
        request.linkname = nonempty(&self.spec.linkname).map(str::to_owned);
        request.cmd = nonempty(&self.spec.command).map(str::to_owned);
        let response = EutilsClient::from_env()
            .elink(&request)
            .await
            .map_err(|error| request_error(error, "source_geo_elink"))?;
        let batch = build_elink_batch(&response)?;
        dataframe_output(ctx, batch, "source_geo_elink")
    }
}

fn build_elink_batch(response: &Value) -> Result<RecordBatch, DagError> {
    struct LinkRow {
        source_uid: String,
        source_database: String,
        destination_database: String,
        linkname: String,
        destination_uid: String,
        score: Option<u64>,
    }
    let mut rows = Vec::new();
    for linkset in response
        .get("linksets")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
    {
        let source_database = linkset
            .get("dbfrom")
            .and_then(Value::as_str)
            .unwrap_or(GEO_DATASETS_DB)
            .to_owned();
        let source_uids = linkset
            .get("ids")
            .and_then(Value::as_array)
            .map(|ids| ids.iter().filter_map(value_string).collect::<Vec<_>>())
            .unwrap_or_default();
        for source_uid in source_uids {
            let link_dbs = linkset
                .get("linksetdbs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if link_dbs.is_empty() {
                rows.push(LinkRow {
                    source_uid,
                    source_database: source_database.clone(),
                    destination_database: String::new(),
                    linkname: String::new(),
                    destination_uid: String::new(),
                    score: None,
                });
                continue;
            }
            for link_db in link_dbs {
                let destination_database = link_db
                    .get("dbto")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let linkname = link_db
                    .get("linkname")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let links = link_db
                    .get("links")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if links.is_empty() {
                    rows.push(LinkRow {
                        source_uid: source_uid.clone(),
                        source_database: source_database.clone(),
                        destination_database: destination_database.clone(),
                        linkname: linkname.clone(),
                        destination_uid: String::new(),
                        score: link_db.get("score").and_then(parse_u64),
                    });
                } else {
                    for link in links {
                        rows.push(LinkRow {
                            source_uid: source_uid.clone(),
                            source_database: source_database.clone(),
                            destination_database: destination_database.clone(),
                            linkname: linkname.clone(),
                            destination_uid: value_string(&link).unwrap_or_default(),
                            score: link.get("score").and_then(parse_u64),
                        });
                    }
                }
            }
        }
    }

    let schema = Arc::new(Schema::new(vec![
        utf8("source_uid"),
        utf8("source_database"),
        utf8("destination_database"),
        utf8("linkname"),
        utf8("destination_uid"),
        uint64("score"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(
                rows.iter()
                    .map(|row| Some(row.source_uid.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.source_database.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.destination_database.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.linkname.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.destination_uid.clone()))
                    .collect(),
            ),
            u64_array(rows.iter().map(|row| row.score).collect()),
        ],
    )
    .map_err(|error| schedule_error("source_geo_elink", error))
}

// ===========================================================================
// EGQuery
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct GeoEGQuerySpec {
    /// Query counted across every Entrez database.
    pub term: String,
}

#[derive(Clone)]
pub struct GeoEGQueryNode {
    meta: NodePorts,
    spec: GeoEGQuerySpec,
}

pub struct GeoEGQueryNodeFactory;

impl_geo_node_factory!(
    GeoEGQueryNodeFactory,
    GeoEGQueryNode,
    GeoEGQuerySpec,
    "source_geo_egquery",
    "Counts a GEO query across every Entrez database.",
    "Wraps the E-utilities EGQuery endpoint. This is useful before a focused \
     GEO DataSets search to see how a term is distributed across Entrez. \
     Output columns are database, menu_name, count, and status."
);

#[async_trait]
impl DagNode for GeoEGQueryNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_geo_egquery"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.spec.term.trim().is_empty() {
            return Err(schedule_error(
                "source_geo_egquery",
                "a non-empty term is required",
            ));
        }
        let response = EutilsClient::from_env()
            .egquery(self.spec.term.trim())
            .await
            .map_err(|error| request_error(error, "source_geo_egquery"))?;
        let batch = build_egquery_batch(&response)?;
        dataframe_output(ctx, batch, "source_geo_egquery")
    }
}

fn build_egquery_batch(response: &EGQueryResponse) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("database"),
        utf8("menu_name"),
        uint64("count"),
        utf8("status"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(
                response
                    .result
                    .iter()
                    .map(|row| row.get("dbname").and_then(value_string))
                    .collect(),
            ),
            str_array(
                response
                    .result
                    .iter()
                    .map(|row| row.get("menuname").and_then(value_string))
                    .collect(),
            ),
            u64_array(
                response
                    .result
                    .iter()
                    .map(|row| row.get("count").and_then(parse_u64))
                    .collect(),
            ),
            str_array(
                response
                    .result
                    .iter()
                    .map(|row| row.get("status").and_then(value_string))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_geo_egquery", error))
}

// ===========================================================================
// ESpell
// ===========================================================================

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct GeoESpellSpec {
    /// Entrez database to spell-check against. Defaults to `gds`.
    #[serde(default)]
    pub database: Option<String>,
    /// Query text to check.
    pub term: String,
}

#[derive(Clone)]
pub struct GeoESpellNode {
    meta: NodePorts,
    spec: GeoESpellSpec,
}

pub struct GeoESpellNodeFactory;

impl_geo_node_factory!(
    GeoESpellNodeFactory,
    GeoESpellNode,
    GeoESpellSpec,
    "source_geo_espell",
    "Returns GEO query spelling corrections as a table.",
    "Wraps the E-utilities ESpell endpoint. Output columns are database, \
     original_query, corrected_query, and the complete NCBI response as JSON."
);

#[async_trait]
impl DagNode for GeoESpellNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_geo_espell"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if self.spec.term.trim().is_empty() {
            return Err(schedule_error(
                "source_geo_espell",
                "a non-empty term is required",
            ));
        }
        let database = nonempty(&self.spec.database).unwrap_or(GEO_DATASETS_DB);
        let response = EutilsClient::from_env()
            .espell(database, self.spec.term.trim())
            .await
            .map_err(|error| request_error(error, "source_geo_espell"))?;
        let batch = build_espell_batch(database, &response)?;
        dataframe_output(ctx, batch, "source_geo_espell")
    }
}

fn build_espell_batch(database: &str, response: &Value) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("database"),
        utf8("original_query"),
        utf8("corrected_query"),
        utf8("response_json"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(database.to_owned())]),
            str_array(vec![
                response
                    .pointer("/OriginalQuery/term")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ]),
            str_array(vec![
                response
                    .pointer("/CorrectedQuery/term")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ]),
            str_array(vec![json_string(response)]),
        ],
    )
    .map_err(|error| schedule_error("source_geo_espell", error))
}

fn nonempty(value: &Option<String>) -> Option<&str> {
    let value = value.as_deref()?.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geo_search_spec_maps_endpoint_filters() {
        let spec = GeoESearchSpec {
            accession: Some("GSE48094".into()),
            retmax: Some(5),
            retstart: Some(10),
            sort: Some("relevance".into()),
            usehistory: Some(true),
            ..Default::default()
        };
        let request = geo_search_request(&spec).unwrap();
        assert_eq!(
            request.into_esearch_request().unwrap().term,
            "GSE48094[ACCN] AND GSE[ETYP]"
        );
    }

    #[test]
    fn geo_search_spec_rejects_unknown_entry_type() {
        let spec = GeoESearchSpec {
            query: Some("single cell".into()),
            entry_type: Some("GXX".into()),
            ..Default::default()
        };
        assert!(geo_search_request(&spec).is_err());
    }

    #[test]
    fn parses_egquery_batch() {
        let response = serde_json::from_value::<EGQueryResponse>(serde_json::json!({
            "result": [
                {"dbname": "gds", "menuname": "GEO DataSets", "count": "8", "status": "Ok"}
            ]
        }))
        .unwrap();
        let batch = build_egquery_batch(&response).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 4);
    }

    #[test]
    fn parses_elink_batch() {
        let response = serde_json::json!({
            "linksets": [{
                "dbfrom": "gds",
                "ids": ["200048094"],
                "linksetdbs": [{
                    "dbto": "pubmed",
                    "linkname": "gds_pubmed",
                    "links": ["23946636"]
                }]
            }]
        });
        let batch = build_elink_batch(&response).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 6);
    }

    #[test]
    fn parses_espell_batch() {
        let response = serde_json::json!({
            "OriginalQuery": {"term": "onkogene"},
            "CorrectedQuery": {"term": "oncogene"}
        });
        let batch = build_espell_batch("gds", &response).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 4);
    }
}
