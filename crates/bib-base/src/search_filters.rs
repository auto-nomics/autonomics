//! Exact metadata filters and disjunctive, parent-article facet counts.
//! This runs in the same read snapshot as lexical search, before pagination.
use crate::{
    Error, Result,
    bib_base::{BibCore, SearchCandidate},
};
use bib_types::{SearchFacetValue, SearchFacets, SearchFilters, SearchFulltextStatus};
use std::collections::{BTreeMap, HashMap, HashSet};
use turso::{Connection, Value};

#[derive(Default)]
pub(crate) struct MetadataCache {
    revision: Option<(u64, String)>,
    documents: HashMap<String, Metadata>,
}

struct Metadata {
    id: String,
    library: i64,
    year: Option<u16>,
    types: Vec<String>,
    tags: Vec<(i64, String)>,
    creators: Vec<(i64, String)>,
    fulltext: bool,
    annotations: bool,
    statuses: Vec<SearchFulltextStatus>,
}
impl Metadata {
    fn matches(&self, f: &SearchFilters, except: &str) -> bool {
        (except == "tags"
            || f.tag_ids.is_empty()
            || self.tags.iter().any(|(id, _)| f.tag_ids.contains(id)))
            && (except == "creators"
                || f.creator_ids.is_empty()
                || self
                    .creators
                    .iter()
                    .any(|(id, _)| f.creator_ids.contains(id)))
            && (except == "types"
                || f.publication_types.is_empty()
                || self
                    .types
                    .iter()
                    .any(|name| f.publication_types.contains(name)))
            && (except == "years"
                || (f
                    .year_from
                    .is_none_or(|from| self.year.is_some_and(|y| y >= from))
                    && f.year_to
                        .is_none_or(|to| self.year.is_some_and(|y| y <= to))))
            && f.has_fulltext.is_none_or(|value| self.fulltext == value)
            && f.has_annotations
                .is_none_or(|value| self.annotations == value)
            && (except == "status"
                || f.fulltext_status
                    .is_none_or(|status| self.statuses.contains(&status)))
    }
}

pub(crate) fn validate(f: &SearchFilters) -> Result<()> {
    let invalid = |message: &str| crate::Error::InvalidSearchQuery(message.into());
    if f.tag_ids.len() > 50 || f.creator_ids.len() > 50 || f.publication_types.len() > 50 {
        return Err(invalid("at most 50 selections are allowed per filter"));
    }
    if f.tag_ids.iter().chain(&f.creator_ids).any(|id| *id <= 0) {
        return Err(invalid("tag and creator IDs must be positive"));
    }
    if f.publication_types
        .iter()
        .any(|v| v.trim().is_empty() || v.len() > 256)
    {
        return Err(invalid("publication types must contain 1–256 bytes"));
    }
    if f.year_from.zip(f.year_to).is_some_and(|(a, b)| a > b) {
        return Err(invalid("year_from must not exceed year_to"));
    }
    Ok(())
}

pub(crate) async fn apply(
    core: &BibCore,
    hits: &mut Vec<SearchCandidate>,
    f: &SearchFilters,
    library: Option<i64>,
    include_facets: bool,
) -> Result<Option<SearchFacets>> {
    if *f == SearchFilters::default() && library.is_none() && !include_facets {
        return Ok(None);
    }
    let (generation, mut cache) = {
        let mut index = core
            .search_index
            .lock()
            .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?;
        (
            (
                index.revision.unwrap_or(0),
                index.commit_token.clone().unwrap_or_default(),
            ),
            std::mem::take(&mut index.filter_cache),
        )
    };
    let result = cache
        .apply(&core.conn(), hits, f, library, include_facets, generation)
        .await;
    core.search_index
        .lock()
        .map_err(|_| Error::Unknown("search worker lock poisoned".into()))?
        .filter_cache = cache;
    result
}
impl MetadataCache {
    async fn apply(
        &mut self,
        conn: &Connection,
        hits: &mut Vec<SearchCandidate>,
        f: &SearchFilters,
        library: Option<i64>,
        include_facets: bool,
        generation: (u64, String),
    ) -> Result<Option<SearchFacets>> {
        if self.revision.as_ref() != Some(&generation) {
            match self.revision.as_ref() {
                Some((previous, _)) if *previous < generation.0 => {
                    let mut rows = conn
                        .query(
                            "SELECT articleID FROM searchOutbox WHERE revision>?1 AND revision<=?2",
                            turso::params![*previous as i64, generation.0 as i64],
                        )
                        .await?;
                    while let Some(row) = rows.next().await? {
                        self.documents.remove(&row.get::<String>(0)?);
                    }
                }
                _ => self.documents.clear(),
            }
            self.revision = Some(generation);
        }
        let missing = hits
            .iter()
            .filter(|h| !self.documents.contains_key(&h.article_id))
            .map(|h| h.article_id.clone())
            .collect::<Vec<_>>();
        for chunk in missing.chunks(200) {
            let placeholders = (1..=chunk.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let params: Vec<Value> = chunk.iter().map(|id| Value::Text(id.clone())).collect();
            let mut rows=conn.query(format!("SELECT a.id,i.libraryID,a.year,COALESCE(a.pub_types,'[]'), \
            COALESCE((SELECT json_group_array(json_array(t.tagID,t.name)) FROM itemTags it JOIN tags t ON t.tagID=it.tagID WHERE it.itemID=i.itemID),'[]'), \
            COALESCE((SELECT json_group_array(json_array(c.creatorID,trim(COALESCE(c.firstName,'') || ' ' || COALESCE(c.lastName,'')))) FROM itemCreators ic JOIN creators c ON c.creatorID=ic.creatorID WHERE ic.itemID=i.itemID),'[]'), \
            EXISTS(SELECT 1 FROM itemAttachments ia LEFT JOIN fulltextItems f ON f.itemID=ia.itemID WHERE ia.parentItemID=i.itemID AND (length(COALESCE(f.textContent,''))>0 OR (ia.linkMode<>3 AND (length(COALESCE(ia.path,''))>0 OR length(COALESCE(ia.vfsPath,''))>0)))), \
            EXISTS(SELECT 1 FROM annotations n WHERE n.article_id=a.id), \
            EXISTS(SELECT 1 FROM itemAttachments ia JOIN fulltextItems f ON f.itemID=ia.itemID WHERE ia.parentItemID=i.itemID AND length(COALESCE(f.textContent,''))>0), \
            EXISTS(SELECT 1 FROM itemAttachments ia JOIN fulltextItems f ON f.itemID=ia.itemID WHERE ia.parentItemID=i.itemID AND f.extractionStatus='failed'), \
            EXISTS(SELECT 1 FROM itemAttachments ia LEFT JOIN fulltextItems f ON f.itemID=ia.itemID WHERE ia.parentItemID=i.itemID AND ia.linkMode<>3 AND (length(COALESCE(ia.path,''))>0 OR length(COALESCE(ia.vfsPath,''))>0) AND length(COALESCE(f.textContent,''))=0 AND COALESCE(f.extractionStatus,'pending')<>'failed') \
            FROM articles a JOIN articleItems m ON m.article_id=a.id JOIN items i ON i.itemID=m.itemID WHERE a.id IN ({placeholders})"),
            turso::params_from_iter(params)).await?;
            while let Some(row) = rows.next().await? {
                let fulltext = row.get::<i64>(6)? != 0;
                let indexed = row.get::<i64>(8)? != 0;
                let failed = row.get::<i64>(9)? != 0;
                let pending = row.get::<i64>(10)? != 0;
                let mut statuses = Vec::new();
                if indexed {
                    statuses.push(SearchFulltextStatus::Indexed);
                }
                if failed {
                    statuses.push(SearchFulltextStatus::Failed);
                }
                if pending {
                    statuses.push(SearchFulltextStatus::Pending);
                }
                if statuses.is_empty() {
                    statuses.push(SearchFulltextStatus::Missing);
                }
                let metadata = Metadata {
                    id: row.get(0)?,
                    library: row.get(1)?,
                    year: row.get::<i64>(2).ok().and_then(|v| u16::try_from(v).ok()),
                    types: serde_json::from_str(&row.get::<String>(3)?)?,
                    tags: serde_json::from_str(&row.get::<String>(4)?)?,
                    creators: serde_json::from_str(&row.get::<String>(5)?)?,
                    fulltext,
                    annotations: row.get::<i64>(7)? != 0,
                    statuses,
                };
                self.documents.insert(metadata.id.clone(), metadata);
            }
        }
        let metadata = hits
            .iter()
            .filter_map(|hit| self.documents.get(&hit.article_id))
            .filter(|m| library.is_none_or(|id| m.library == id))
            .collect::<Vec<_>>();
        let facets = include_facets.then(|| facets(&metadata, f));
        let allowed: HashSet<_> = metadata
            .iter()
            .filter(|m| m.matches(f, ""))
            .map(|m| m.id.as_str())
            .collect();
        hits.retain(|h| allowed.contains(h.article_id.as_str()));
        Ok(facets)
    }
}

type Counts = BTreeMap<String, (String, usize)>;
fn add(counts: &mut Counts, value: String, label: String) {
    let entry = counts.entry(value).or_insert((label, 0));
    entry.1 += 1;
}
fn values(counts: Counts) -> Vec<SearchFacetValue> {
    let mut values: Vec<_> = counts
        .into_iter()
        .map(|(value, (label, count))| SearchFacetValue {
            value,
            label,
            count,
        })
        .collect();
    values.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.value.cmp(&b.value))
    });
    values
}
fn facets(docs: &[&Metadata], f: &SearchFilters) -> SearchFacets {
    let mut counts: HashMap<&str, Counts> = HashMap::new();
    for doc in docs {
        for kind in ["tags", "creators", "types", "years", "status"] {
            if !doc.matches(f, kind) {
                continue;
            }
            let counts = counts.entry(kind).or_default();
            match kind {
                "tags" | "creators" => {
                    let entries = if kind == "tags" {
                        &doc.tags
                    } else {
                        &doc.creators
                    };
                    let mut seen = HashSet::new();
                    for (id, label) in entries {
                        if seen.insert(*id) {
                            add(counts, id.to_string(), label.clone());
                        }
                    }
                }
                "types" => {
                    for label in doc.types.iter().collect::<HashSet<_>>() {
                        add(counts, label.clone(), label.clone());
                    }
                }
                "years" => {
                    let label = doc
                        .year
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "unknown".into());
                    add(counts, label.clone(), label);
                }
                _ => {
                    for status in &doc.statuses {
                        let label = match status {
                            SearchFulltextStatus::Indexed => "indexed",
                            SearchFulltextStatus::Pending => "pending",
                            SearchFulltextStatus::Failed => "failed",
                            SearchFulltextStatus::Missing => "missing",
                        };
                        add(counts, label.into(), label.into());
                    }
                }
            }
        }
    }
    SearchFacets {
        tags: values(counts.remove("tags").unwrap_or_default()),
        creators: values(counts.remove("creators").unwrap_or_default()),
        publication_types: values(counts.remove("types").unwrap_or_default()),
        years: values(counts.remove("years").unwrap_or_default()),
        fulltext_status: values(counts.remove("status").unwrap_or_default()),
    }
}
