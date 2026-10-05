//! `source_gwascatalog_studies` — curated GWAS Catalog studies as a table.

use std::sync::Arc;

use arrow_array::{BooleanArray, RecordBatch, UInt64Array};
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util;
use crate::nodes::{joined, str_array, utf8};
use crate::rest::{RestPage, RestStudy, StudyQuery};

/// Spec for [`StudiesNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct StudiesSpec {
    /// Study accession ID, e.g. `GCST005038`.
    #[serde(default)]
    pub accession_id: Option<String>,
    /// EFO trait label to search by, e.g. `body mass index`.
    #[serde(default)]
    pub efo_trait: Option<String>,
    /// EFO ontology URI, e.g. `http://www.ebi.ac.uk/efo/EFO_0004343`.
    #[serde(default)]
    pub efo_uri: Option<String>,
    /// Reported disease trait name.
    #[serde(default)]
    pub disease_trait: Option<String>,
    /// PubMed ID to find associated studies.
    #[serde(default)]
    pub pubmed_id: Option<String>,
    /// Filter by user-requested status (true/false).
    #[serde(default)]
    pub user_requested: Option<bool>,
    /// Filter by full p-value set availability (true/false).
    #[serde(default)]
    pub full_pvalue_set: Option<bool>,
    /// Page size per request (default 20, max 1000). With no filter this
    /// lists all curated studies from page 0.
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows to fetch across pages. Default: one page.
    #[serde(default)]
    pub max_results: Option<u32>,
}

/// Build the [`StudyQuery`] for a spec, failing when filters conflict (the
/// REST API serves one `findBy*` endpoint per request).
pub(crate) fn study_query(spec: &StudiesSpec) -> Result<Option<StudyQuery>, DagError> {
    let filters: Vec<(&str, StudyQuery)> = [
        spec.accession_id
            .as_deref()
            .map(|v| ("accession_id", StudyQuery::Accession(v.to_string()))),
        spec.efo_trait
            .as_deref()
            .map(|v| ("efo_trait", StudyQuery::EfoTrait(v.to_string()))),
        spec.efo_uri
            .as_deref()
            .map(|v| ("efo_uri", StudyQuery::EfoUri(v.to_string()))),
        spec.disease_trait
            .as_deref()
            .map(|v| ("disease_trait", StudyQuery::DiseaseTrait(v.to_string()))),
        spec.pubmed_id
            .as_deref()
            .map(|v| ("pubmed_id", StudyQuery::Pmid(v.to_string()))),
        spec.user_requested
            .map(|v| ("user_requested", StudyQuery::UserRequested(v))),
        spec.full_pvalue_set
            .map(|v| ("full_pvalue_set", StudyQuery::FullPvalueSet(v))),
    ]
    .into_iter()
    .flatten()
    .collect();
    match filters.as_slice() {
        [] => Ok(None),
        [(_, only)] => Ok(Some(only.clone())),
        many => Err(DagError::Schedule(format!(
            "source_gwascatalog_studies accepts at most one search filter per request \
             (the REST API has one findBy* endpoint each); got {} — {}",
            many.len(),
            many.iter()
                .map(|(name, _)| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Fetch studies across pages until `max_results` or the last page.
pub(crate) async fn fetch_studies(
    client: &crate::GwasCatalogClient,
    query: Option<&StudyQuery>,
    page_size: u32,
    max_results: u32,
) -> Result<Vec<RestStudy>, DagError> {
    let mut out = Vec::new();
    let mut page: Option<u32> = None;
    loop {
        let resp: RestPage<crate::rest::EmbeddedRestStudies> = match query {
            Some(q) => client.rest_find_studies(q, page, Some(page_size)).await,
            None => client.rest_studies(page, Some(page_size)).await,
        }
        .map_err(|e| DagError::Schedule(format!("GWAS Catalog studies request failed: {e}")))?;
        let rows = resp
            ._embedded
            .map(|embedded| embedded.studies)
            .unwrap_or_default();
        let total_pages = resp.page.total_pages;
        // An empty page means the filter matched nothing more — without this
        // guard a missing `page` envelope would loop forever.
        if rows.is_empty() {
            return Ok(out);
        }
        out.extend(rows);
        if out.len() >= max_results as usize {
            out.truncate(max_results as usize);
            return Ok(out);
        }
        let next = resp.page.number.saturating_add(1);
        if total_pages > 0 && next >= total_pages {
            return Ok(out);
        }
        page = Some(next);
    }
}

/// Source node emitting curated studies.
#[derive(Clone)]
pub struct StudiesNode {
    meta: NodePorts,
    spec: StudiesSpec,
}

pub struct StudiesNodeFactory;

impl NodeFactory for StudiesNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_studies"
    }

    fn desc(&self) -> &'static str {
        "Search GWAS Catalog curated studies and emit a table."
    }

    fn doc(&self) -> &'static str {
        "A source node over the GWAS Catalog REST API (`/studies`). With no \
         filter it lists curated studies from the first page; pass exactly one \
         of `accession_id`, `efo_trait`, `efo_uri`, `disease_trait`, `pubmed_id`, \
         `user_requested`, or `full_pvalue_set` to use the corresponding \
         findBy* endpoint (the API serves one criterion per request).\n\n\
         `page_size` sets rows per request (default 20); `max_results` \
         auto-paginates up to that many rows total (default: one page).\n\n\
         Output schema: `accession_id, title, pubmed_id, publication_date, \
         journal, first_author, disease_trait, initial_sample_size, \
         replication_sample_size, sample_n, snp_count, full_pvalue_set, \
         imputed, pooled, gxe, gxg, genotyping_technologies, \
         ancestral_groups`.\n\n\
         Pipe into `sql` / `dataframe_to_file` for downstream processing; \
         for harmonised per-variant statistics use \
         `source_gwascatalog_summary_associations`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StudiesSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: StudiesSpec = serde_json::from_value(spec)?;
        Ok(Box::new(StudiesNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for StudiesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_studies"
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
        let query = study_query(&self.spec)?;
        let page_size = self.spec.page_size.unwrap_or(20).min(1000);
        let max_results = self.spec.max_results.unwrap_or(page_size).max(1);
        let client = crate::GwasCatalogClient::new();
        let rows = fetch_studies(&client, query.as_ref(), page_size, max_results).await?;

        let batch = build_studies_batch(&rows)?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read studies batch: {e}")))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

/// Arrow batch builder for study rows.
pub(crate) fn build_studies_batch(rows: &[RestStudy]) -> Result<RecordBatch, DagError> {
    let accession: Vec<_> = rows.iter().map(|s| Some(s.accession_id.clone())).collect();
    let titles: Vec<_> = rows
        .iter()
        .map(|s| s.publication_info.as_ref().and_then(|p| p.title.clone()))
        .collect();
    let pmids: Vec<_> = rows
        .iter()
        .map(|s| {
            s.publication_info
                .as_ref()
                .and_then(|p| p.pubmed_id.clone())
        })
        .collect();
    let pub_dates: Vec<_> = rows
        .iter()
        .map(|s| {
            s.publication_info
                .as_ref()
                .and_then(|p| p.publication_date.clone())
        })
        .collect();
    let journals: Vec<_> = rows
        .iter()
        .map(|s| {
            s.publication_info
                .as_ref()
                .and_then(|p| p.publication.clone())
        })
        .collect();
    let authors: Vec<_> = rows
        .iter()
        .map(|s| {
            s.publication_info
                .as_ref()
                .and_then(|p| p.author.as_ref()?.fullname.clone())
        })
        .collect();
    let traits: Vec<_> = rows
        .iter()
        .map(|s| {
            let t = &s.disease_trait.as_ref()?.trait_name;
            (!t.is_empty()).then(|| t.clone())
        })
        .collect();
    let initial: Vec<_> = rows
        .iter()
        .map(|s| (!s.initial_sample_size.is_empty()).then(|| s.initial_sample_size.clone()))
        .collect();
    let replication: Vec<_> = rows
        .iter()
        .map(|s| (!s.replication_sample_size.is_empty()).then(|| s.replication_sample_size.clone()))
        .collect();
    let sample_n: Vec<Option<u64>> = rows
        .iter()
        .map(|s| {
            let n: u64 = s.ancestries.iter().map(|a| a.number_of_individuals).sum();
            (n > 0).then_some(n)
        })
        .collect();
    let snp_counts: Vec<Option<u64>> = rows
        .iter()
        .map(|s| (s.snp_count > 0).then_some(s.snp_count))
        .collect();
    let full_pvalue: Vec<Option<bool>> = rows.iter().map(|s| Some(s.full_pvalue_set)).collect();
    let imputed: Vec<Option<bool>> = rows.iter().map(|s| Some(s.imputed)).collect();
    let pooled: Vec<Option<bool>> = rows.iter().map(|s| Some(s.pooled)).collect();
    let gxe: Vec<Option<bool>> = rows.iter().map(|s| Some(s.gxe)).collect();
    let gxg: Vec<Option<bool>> = rows.iter().map(|s| Some(s.gxg)).collect();
    let genotyping: Vec<_> = rows
        .iter()
        .map(|s| {
            joined(
                &s.genotyping_technologies
                    .iter()
                    .filter_map(|t| t.genotyping_technology.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let groups: Vec<_> = rows
        .iter()
        .map(|s| {
            joined(
                &s.ancestries
                    .iter()
                    .flat_map(|a| {
                        a.ancestral_groups
                            .iter()
                            .filter_map(|g| g.ancestral_group.clone())
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();

    let schema = Arc::new(Schema::new(vec![
        crate::nodes::utf8("accession_id"),
        crate::nodes::utf8("title"),
        crate::nodes::utf8("pubmed_id"),
        crate::nodes::utf8("publication_date"),
        crate::nodes::utf8("journal"),
        crate::nodes::utf8("first_author"),
        crate::nodes::utf8("disease_trait"),
        crate::nodes::utf8("initial_sample_size"),
        crate::nodes::utf8("replication_sample_size"),
        arrow_schema::Field::new("sample_n", arrow_schema::DataType::UInt64, true),
        arrow_schema::Field::new("snp_count", arrow_schema::DataType::UInt64, true),
        arrow_schema::Field::new("full_pvalue_set", arrow_schema::DataType::Boolean, true),
        arrow_schema::Field::new("imputed", arrow_schema::DataType::Boolean, true),
        arrow_schema::Field::new("pooled", arrow_schema::DataType::Boolean, true),
        arrow_schema::Field::new("gxe", arrow_schema::DataType::Boolean, true),
        arrow_schema::Field::new("gxg", arrow_schema::DataType::Boolean, true),
        crate::nodes::utf8("genotyping_technologies"),
        crate::nodes::utf8("ancestral_groups"),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            crate::nodes::str_array(accession),
            crate::nodes::str_array(titles),
            crate::nodes::str_array(pmids),
            crate::nodes::str_array(pub_dates),
            crate::nodes::str_array(journals),
            crate::nodes::str_array(authors),
            crate::nodes::str_array(traits),
            crate::nodes::str_array(initial),
            crate::nodes::str_array(replication),
            Arc::new(UInt64Array::from(sample_n)),
            Arc::new(UInt64Array::from(snp_counts)),
            Arc::new(BooleanArray::from(full_pvalue)),
            Arc::new(BooleanArray::from(imputed)),
            Arc::new(BooleanArray::from(pooled)),
            Arc::new(BooleanArray::from(gxe)),
            Arc::new(BooleanArray::from(gxg)),
            crate::nodes::str_array(genotyping),
            crate::nodes::str_array(groups),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build studies batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn study(accession: &str, title: &str) -> RestStudy {
        let mut s = RestStudy {
            accession_id: accession.into(),
            snp_count: 2_450_000,
            full_pvalue_set: true,
            ..Default::default()
        };
        s.publication_info = Some(crate::rest::PublicationInfo {
            title: Some(title.into()),
            pubmed_id: Some("12345678".into()),
            publication: Some("Nature Genetics".into()),
            publication_date: Some("2023-05-01".into()),
            author: Some(crate::rest::PublicationAuthor {
                fullname: Some("Smith J".into()),
                orcid: None,
            }),
        });
        s.disease_trait = Some(crate::rest::DiseaseTrait {
            trait_name: "body mass index".into(),
        });
        s.initial_sample_size = "461,460 European ancestry individuals".into();
        s
    }

    #[test]
    fn batch_carries_publication_and_sample_columns() {
        let batch = build_studies_batch(&[study("GCST005038", "GWAS of BMI")]).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 18);
        let pmid = batch
            .column(2)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(pmid.value(0), "12345678");
        let snps = batch
            .column(10)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(snps.value(0), 2_450_000);
    }

    #[test]
    fn zero_rows_builds_an_empty_typed_batch() {
        let batch = build_studies_batch(&[]).unwrap();
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.num_columns(), 18);
    }

    #[test]
    fn at_most_one_filter_is_accepted() {
        let mut spec = StudiesSpec {
            accession_id: Some("GCST005038".into()),
            efo_trait: None,
            efo_uri: None,
            disease_trait: None,
            pubmed_id: None,
            user_requested: None,
            full_pvalue_set: None,
            page_size: None,
            max_results: None,
        };
        assert!(study_query(&spec).unwrap().is_some());
        spec.efo_trait = Some("body mass index".into());
        let err = study_query(&spec).unwrap_err().to_string();
        assert!(err.contains("at most one search filter"), "{err}");
        assert!(err.contains("`accession_id`"), "{err}");
        assert!(err.contains("`efo_trait`"), "{err}");
    }
}
