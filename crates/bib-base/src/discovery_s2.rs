//! Semantic Scholar endpoint-aware discovery and bounded bulk-batch paging.
use crate::discovery_sources::{
    ProviderCursor, cursor_decode, cursor_encode, status_failure, validate_page,
};
use bib_types::{Article, Identifier, SearchExpr, SearchField, discovery::*};
use semantic_scholar::{Paper, PaperSearchFilter, S2Client, S2Error};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, SourceFailure>;
const SOURCE: &str = "semantic_scholar";
const WINDOW: u64 = 10_000_000;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
fn invalid(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::InvalidQuery, message, false)
}
fn unsupported(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::Unsupported, message, false)
}
fn bad(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::InvalidResponse, message, false)
}
fn expired(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::CursorExpired, message, false)
}

pub(crate) fn capabilities(client: &S2Client) -> SourceCapabilities {
    SourceCapabilities {
        source:SOURCE.into(),searchable:true,native_query:true,
        fields:vec![SearchField::Any,SearchField::Identifier,SearchField::Publication,SearchField::Year],
        filters:vec!["year".into(),"publication_type".into()],
        sorts:vec![DiscoverySort::Relevance,DiscoverySort::Newest,DiscoverySort::Oldest,DiscoverySort::CitationCount],
        pagination:SourcePagination::Cursor,max_page_size:100,credentials_configured:client.has_api_key(),
        authentication:SourceAuthentication::Optional,quota:None,
        notes:vec!["Bulk queries match title/abstract with Boolean syntax and have no relevance ranking; default strict bulk order is paperId. Relevance search is an explicit approximate text mode limited to 1000 results. Title/abstract-only hints require approximate matching; author-name constraints are unsupported.".into(),"Native endpoint selects bulk/relevance/lookup. Public PDF presence is available as native openAccessPdf, not a generic OA predicate. Bulk totals are estimates and continuation is not an upstream snapshot.".into()],
    }
}

fn year(value: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .ok()
        .filter(|value| (1..=9999).contains(value))
        .ok_or_else(|| invalid("invalid Semantic Scholar publication year"))
}
fn bound(target: &mut Option<u16>, value: u16, lower: bool) {
    *target = Some(if lower {
        target.unwrap_or(1).max(value)
    } else {
        target.unwrap_or(9999).min(value)
    });
}
pub(crate) fn identifier(value: &str) -> Result<String> {
    let value = value.trim();
    if let Some(key) = crate::discovery_merge::identifier_key(&Identifier::doi(value)) {
        return Ok(format!("DOI:{}", &key[4..]));
    }
    if let Some(id) = crate::discovery_arxiv::identifier(value) {
        if id != crate::discovery_arxiv::version_base(&id) {
            return Err(unsupported(
                "Semantic Scholar cannot guarantee an exact arXiv version; use the arXiv source",
            ));
        }
        return Ok(format!("ARXIV:{id}"));
    }
    let (prefix, suffix) = value.split_once(':').unwrap_or(("s2", value));
    if value.len() < 40 && !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value
            .trim_start_matches('0')
            .parse::<u64>()
            .map(|id| format!("PMID:{id}"))
            .map_err(|_| invalid("invalid PMID"));
    }
    match prefix.to_ascii_lowercase().as_str() {
        "s2" if suffix.len() == 40 && suffix.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(suffix.to_ascii_lowercase())
        }
        "pmid" if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) => Ok(format!(
            "PMID:{}",
            suffix
                .trim_start_matches('0')
                .parse::<u64>()
                .map_err(|_| invalid("invalid PMID"))?
        )),
        "corpusid" if suffix.parse::<i64>().ok().is_some_and(|value| value > 0) => {
            Ok(format!("CorpusId:{}", suffix.parse::<i64>().unwrap()))
        }
        _ => Err(unsupported(
            "Semantic Scholar exact lookup accepts DOI, PMID, unversioned arXiv, S2 hash or CorpusId only",
        )),
    }
}
fn publication_type(value: &str) -> Result<&'static str> {
    let key = value.to_ascii_lowercase().replace(['_', '-', ' '], "");
    Ok(match key.as_str() {
        "article" | "journalarticle" => "JournalArticle",
        "review" => "Review",
        "casereport" => "CaseReport",
        "clinicaltrial" => "ClinicalTrial",
        "conference" => "Conference",
        "dataset" => "Dataset",
        "editorial" => "Editorial",
        "lettersandcomments" => "LettersAndComments",
        "metaanalysis" => "MetaAnalysis",
        "news" => "News",
        "study" => "Study",
        "book" => "Book",
        "booksection" | "chapter" => "BookSection",
        _ => {
            return Err(unsupported(
                "Semantic Scholar cannot preserve this publication type",
            ));
        }
    })
}
#[derive(Default)]
struct Clauses {
    text: Vec<SearchExpr>,
    ids: Vec<String>,
    venues: Vec<BTreeSet<String>>,
    from: Option<u16>,
    until: Option<u16>,
}
fn venue_union(expr: &SearchExpr) -> Option<BTreeSet<String>> {
    match expr {
        SearchExpr::Term {
            value,
            field: Some(SearchField::Publication),
        }
        | SearchExpr::Phrase {
            value,
            field: Some(SearchField::Publication),
        } => Some([value.clone()].into()),
        SearchExpr::Or { clauses } => clauses
            .iter()
            .map(venue_union)
            .collect::<Option<Vec<_>>>()
            .map(|sets| sets.into_iter().flatten().collect()),
        _ => None,
    }
}
fn collect(expr: &SearchExpr, out: &mut Clauses) -> Result<()> {
    match expr {
        SearchExpr::And { clauses } => {
            for child in clauses {
                collect(child, out)?;
            }
        }
        SearchExpr::Term {
            value,
            field: Some(SearchField::Identifier),
        }
        | SearchExpr::Phrase {
            value,
            field: Some(SearchField::Identifier),
        } => out.ids.push(identifier(value)?),
        SearchExpr::Term {
            value,
            field: Some(SearchField::Year),
        }
        | SearchExpr::Phrase {
            value,
            field: Some(SearchField::Year),
        } => {
            let value = year(value)?;
            bound(&mut out.from, value, true);
            bound(&mut out.until, value, false);
        }
        SearchExpr::Range {
            field: SearchField::Year,
            gte,
            lte,
        } => {
            if let Some(value) = gte {
                bound(&mut out.from, year(value)?, true);
            }
            if let Some(value) = lte {
                bound(&mut out.until, year(value)?, false);
            }
        }
        _ => {
            if let Some(venues) = venue_union(expr) {
                out.venues.push(venues);
            } else {
                out.text.push(expr.clone());
            }
        }
    }
    Ok(())
}

// Bounded DNF makes negation scopes explicit. Each union branch must contain
// a positive text anchor; the upstream API has no proven all-documents term.
type Dnf = Vec<Vec<(String, bool)>>;
fn dnf(expr: &SearchExpr, negate: bool, approximate: bool) -> Result<Dnf> {
    match expr {
        SearchExpr::Not { clause } => dnf(clause, !negate, approximate),
        SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
            let intersection = matches!(expr, SearchExpr::And { .. }) != negate;
            let mut out: Dnf = if intersection {
                vec![Vec::new()]
            } else {
                Vec::new()
            };
            for child in clauses {
                let next = dnf(child, negate, approximate)?;
                if intersection {
                    if out.len().saturating_mul(next.len()) > 64 {
                        return Err(unsupported(
                            "Semantic Scholar Boolean expansion exceeds 64 branches",
                        ));
                    }
                    out = out
                        .into_iter()
                        .flat_map(|left| {
                            next.iter()
                                .map(move |right| [left.clone(), right.clone()].concat())
                        })
                        .collect();
                } else {
                    out.extend(next);
                }
                if out.len() > 64 || out.iter().any(|row| row.len() > 128) {
                    return Err(unsupported(
                        "Semantic Scholar Boolean query exceeds the supported complexity",
                    ));
                }
            }
            Ok(out)
        }
        SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field } => {
            match field.unwrap_or(SearchField::Any) {
                SearchField::Any => {}
                SearchField::Title | SearchField::Abstract if approximate => {}
                SearchField::Title | SearchField::Abstract => {
                    return Err(unsupported(
                        "Semantic Scholar searches title and abstract together; field-only text requires allow_approximate",
                    ));
                }
                _ => {
                    return Err(unsupported(
                        "Semantic Scholar cannot preserve this text field or Boolean filter combination",
                    ));
                }
            }
            if matches!(expr, SearchExpr::Term { .. }) && value.split_whitespace().count() > 1 {
                let parts = value
                    .split_whitespace()
                    .map(|value| SearchExpr::Term {
                        value: value.into(),
                        field: *field,
                    })
                    .collect();
                return dnf(&SearchExpr::And { clauses: parts }, negate, approximate);
            }
            Ok(vec![vec![(
                crate::discovery_plan::literal(value)?,
                !negate,
            )]])
        }
        _ => Err(unsupported(
            "Semantic Scholar cannot distribute metadata filters into this Boolean shape",
        )),
    }
}
fn render(expr: &SearchExpr, approximate: bool) -> Result<String> {
    let rows = dnf(expr, false, approximate)?;
    if rows
        .iter()
        .any(|row| !row.iter().any(|(_, positive)| *positive))
    {
        return Err(unsupported(
            "Semantic Scholar exclusions need a positive text anchor in every OR branch",
        ));
    }
    Ok(rows
        .into_iter()
        .map(|mut row| {
            row.sort_by_key(|(_, positive)| !*positive);
            format!(
                "({})",
                row.into_iter()
                    .map(|(value, positive)| if positive { value } else { format!("-{value}") })
                    .collect::<Vec<_>>()
                    .join(" + ")
            )
        })
        .collect::<Vec<_>>()
        .join(" | "))
}
fn loose(expr: &SearchExpr) -> Option<Vec<String>> {
    match expr {
        SearchExpr::And { clauses } | SearchExpr::Or { clauses } => clauses
            .iter()
            .map(loose)
            .collect::<Option<Vec<_>>>()
            .map(|values| values.into_iter().flatten().collect()),
        SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field }
            if matches!(
                field.unwrap_or(SearchField::Any),
                SearchField::Any | SearchField::Title | SearchField::Abstract
            ) && !value.contains('-') =>
        {
            Some(vec![value.clone()])
        }
        _ => None,
    }
}
pub(crate) fn parameters(request: &DiscoveryRequest) -> Result<BTreeMap<String, String>> {
    if request.filters.open_access.is_some() {
        return Err(unsupported(
            "Semantic Scholar openAccessPdf is narrower than generic OA; use its explicit native flag",
        ));
    }
    let mut clauses = Clauses::default();
    if let Some(expr) = &request.expression {
        collect(expr, &mut clauses)?;
    }
    if let Some(value) = request.filters.year_from {
        bound(&mut clauses.from, value, true);
    }
    if let Some(value) = request.filters.year_to {
        bound(&mut clauses.until, value, false);
    }
    if matches!((clauses.from,clauses.until),(Some(a),Some(b)) if a>b) {
        return Err(unsupported("contradictory publication year constraints"));
    }
    let mut out = BTreeMap::new();
    if clauses.from.is_some() || clauses.until.is_some() {
        out.insert(
            "year".into(),
            format!(
                "{}-{}",
                clauses.from.map(|v| v.to_string()).unwrap_or_default(),
                clauses.until.map(|v| v.to_string()).unwrap_or_default()
            ),
        );
    }
    if !request.filters.publication_types.is_empty() {
        out.insert(
            "publicationTypes".into(),
            request
                .filters
                .publication_types
                .iter()
                .map(|kind| publication_type(kind))
                .collect::<Result<Vec<_>>>()?
                .join(","),
        );
    }
    let mut sets = clauses.venues.into_iter();
    if let Some(mut venues) = sets.next() {
        for set in sets {
            venues.retain(|value| set.contains(value));
        }
        if venues.is_empty() {
            return Err(unsupported("contradictory venue constraints"));
        }
        if venues.iter().any(|v| v.contains(',')) {
            return Err(unsupported(
                "a literal venue containing a comma needs source-native syntax",
            ));
        }
        out.insert(
            "venue".into(),
            venues.into_iter().collect::<Vec<_>>().join(","),
        );
    }
    if !clauses.ids.is_empty() {
        clauses.ids.sort();
        clauses.ids.dedup();
        if clauses.ids.len() != 1 || !clauses.text.is_empty() || !out.is_empty() {
            return Err(unsupported(
                "Semantic Scholar exact lookup must be a single identifier without other constraints",
            ));
        }
        return Ok([
            ("endpoint".into(), "lookup".into()),
            ("paper_id".into(), clauses.ids.remove(0)),
        ]
        .into());
    }
    let text = SearchExpr::And {
        clauses: clauses.text,
    };
    let loose = if request.allow_approximate && request.sort == DiscoverySort::Relevance {
        loose(&text).filter(|terms| !terms.is_empty())
    } else {
        None
    };
    let endpoint = if let Some(terms) = loose {
        out.insert("query".into(), terms.join(" "));
        "relevance"
    } else {
        if let SearchExpr::And { clauses } = &text
            && !clauses.is_empty()
        {
            out.insert("query".into(), render(&text, request.allow_approximate)?);
        }
        "bulk"
    };
    if out.is_empty() {
        return Err(invalid(
            "provide Semantic Scholar text or metadata constraints",
        ));
    }
    out.insert("endpoint".into(), endpoint.into());
    Ok(out)
}
pub(crate) fn configure(params: &mut BTreeMap<String, String>, sort: DiscoverySort) -> Result<()> {
    let endpoint = params.get("endpoint").cloned().unwrap_or_else(|| {
        if params.contains_key("paper_id") {
            "lookup".into()
        } else {
            "bulk".into()
        }
    });
    if endpoint == "lookup" {
        if params
            .keys()
            .any(|key| !matches!(key.as_str(), "endpoint" | "paper_id"))
        {
            return Err(invalid(
                "Semantic Scholar lookup cannot include query/filter parameters",
            ));
        }
        let id = identifier(
            params
                .get("paper_id")
                .ok_or_else(|| invalid("lookup requires paper_id"))?,
        )?;
        params.insert("paper_id".into(), id);
    } else {
        if params.contains_key("paper_id") {
            return Err(invalid("paper_id requires lookup endpoint"));
        }
        filter(params)?;
        match endpoint.as_str() {
            "relevance" => {
                if sort != DiscoverySort::Relevance {
                    return Err(unsupported(
                        "Semantic Scholar relevance endpoint cannot apply date/citation ordering; choose bulk",
                    ));
                }
                if params
                    .get("query")
                    .is_none_or(|query| query.trim().is_empty())
                {
                    return Err(invalid("relevance endpoint requires plain query text"));
                }
            }
            "bulk" => {
                params.insert(
                    "sort".into(),
                    match sort {
                        DiscoverySort::Relevance => "paperId:asc",
                        DiscoverySort::Newest => "publicationDate:desc",
                        DiscoverySort::Oldest => "publicationDate:asc",
                        DiscoverySort::CitationCount => "citationCount:desc",
                    }
                    .into(),
                );
            }
            _ => {
                return Err(invalid(
                    "Semantic Scholar endpoint must be bulk, relevance or lookup",
                ));
            }
        }
        if params.iter().all(|(key, value)| {
            matches!(key.as_str(), "endpoint" | "sort") || value.trim().is_empty()
        }) && !params.contains_key("openAccessPdf")
        {
            return Err(invalid("provide a query or source-native metadata filter"));
        }
    }
    params.insert("endpoint".into(), endpoint);
    Ok(())
}
fn filter(params: &BTreeMap<String, String>) -> Result<PaperSearchFilter> {
    if params
        .get("openAccessPdf")
        .is_some_and(|value| !value.is_empty())
    {
        return Err(invalid(
            "openAccessPdf is a presence flag with an empty value",
        ));
    }
    let min_citation_count = params
        .get("minCitationCount")
        .map(|value| {
            value
                .parse::<i32>()
                .ok()
                .filter(|value| *value >= 0)
                .ok_or_else(|| invalid("minCitationCount must be a nonnegative integer"))
        })
        .transpose()?;
    Ok(PaperSearchFilter {
        year: params.get("year").cloned(),
        publication_date_or_year: params.get("publicationDateOrYear").cloned(),
        venue: params.get("venue").cloned(),
        fields_of_study: params.get("fieldsOfStudy").cloned(),
        publication_types: params.get("publicationTypes").cloned(),
        open_access_pdf: params.contains_key("openAccessPdf"),
        min_citation_count,
    })
}
pub(crate) fn notices(params: &BTreeMap<String, String>) -> Vec<SourceNotice> {
    let mut out=vec![SourceNotice{code:"estimated_totals".into(),message:"Semantic Scholar totals are provider estimates. Source pages are not a stable upstream snapshot.".into()}];
    match params.get("endpoint").map(String::as_str) {
        Some("relevance")=>out.push(SourceNotice{code:"approximate_text".into(),message:"Relevance mode uses plain-text title/abstract matching, without strict Boolean/phrase or field-only semantics. Only the first 1000 relevance results are retrievable.".into()}),
        Some("bulk")=>out.push(SourceNotice{code:"bulk_order".into(),message:"Bulk retrieval preserves supported Boolean constraints but has no source relevance ranking. Default order is paper ID; date/citation order is available. Continued retrieval may expire if an evicted batch changes upstream.".into()}),
        _=>{},
    }
    out
}

pub(crate) fn field_notices(request: &DiscoveryRequest) -> Vec<SourceNotice> {
    fn broad(expr: &SearchExpr) -> bool {
        match expr {
            SearchExpr::And { clauses } | SearchExpr::Or { clauses } => clauses.iter().any(broad),
            SearchExpr::Not { clause } => broad(clause),
            SearchExpr::Term { field, .. } | SearchExpr::Phrase { field, .. } => {
                matches!(field, Some(SearchField::Title | SearchField::Abstract))
            }
            _ => false,
        }
    }
    if request.expression.as_ref().is_some_and(broad) {
        vec![SourceNotice{code:"approximate_field_scope".into(),message:"The selected Semantic Scholar endpoint searches title and abstract together; requested title/abstract-only scope is broadened by the explicit approximate-matching option.".into()}]
    } else {
        Vec::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Position {
    Relevance {
        offset: u32,
        total: u64,
    },
    Bulk {
        input_token: Option<String>,
        skip: usize,
        batch_digest: Option<String>,
        obtained: u64,
    },
}
struct Batch {
    articles: Vec<Article>,
    total: u64,
    next: Option<String>,
    digest: String,
    bytes: usize,
    at: Instant,
    fetched_at: String,
}
#[derive(Default)]
struct Cache {
    entries: VecDeque<(String, Arc<Batch>)>,
    bytes: usize,
}
#[derive(Default)]
pub(crate) struct BulkCache(Mutex<Cache>);
impl BulkCache {
    fn get(&self, key: &str) -> Option<Arc<Batch>> {
        let mut cache = self.0.lock().unwrap();
        cache
            .entries
            .retain(|(_, entry)| entry.at.elapsed() <= Duration::from_secs(1800));
        cache.bytes = cache.entries.iter().map(|(_, entry)| entry.bytes).sum();
        let index = cache.entries.iter().position(|(id, _)| id == key)?;
        let entry = cache.entries.remove(index).unwrap();
        let out = entry.1.clone();
        cache.entries.push_back(entry);
        Some(out)
    }
    fn insert(&self, key: String, batch: Arc<Batch>) {
        if batch.bytes > CACHE_BYTES {
            return;
        }
        let mut cache = self.0.lock().unwrap();
        if let Some(index) = cache.entries.iter().position(|(id, _)| id == &key) {
            let (_, old) = cache.entries.remove(index).unwrap();
            cache.bytes -= old.bytes;
        }
        while cache.bytes.saturating_add(batch.bytes) > CACHE_BYTES || cache.entries.len() >= 32 {
            let Some((_, old)) = cache.entries.pop_front() else {
                break;
            };
            cache.bytes -= old.bytes;
        }
        cache.bytes += batch.bytes;
        cache.entries.push_back((key, batch));
    }
}
fn failure(error: S2Error, continuation: bool) -> SourceFailure {
    match error {
        S2Error::RateLimited {
            retry_after_seconds,
        } => {
            let mut error = status_failure("Semantic Scholar", 429);
            error.retry_after_seconds = retry_after_seconds;
            error
        }
        S2Error::Status {
            status: 400 | 404 | 410,
            ..
        } if continuation => {
            expired("Semantic Scholar rejected the saved continuation; start a new search")
        }
        S2Error::Status { status, .. } => status_failure("Semantic Scholar", status),
        S2Error::Http(error) => SourceFailure::new(
            if error.is_timeout() {
                SourceFailureKind::Timeout
            } else {
                SourceFailureKind::Transport
            },
            "Semantic Scholar HTTP request failed",
            true,
        ),
        S2Error::Param(_) => invalid("Semantic Scholar rejected parameters or request length"),
        S2Error::Parse(_) | S2Error::InvalidResponse(_) => {
            bad("Semantic Scholar returned invalid metadata")
        }
        S2Error::ResponseTooLarge => {
            bad("Semantic Scholar response exceeds the supported size limit")
        }
    }
}
fn valid(paper: &Paper) -> bool {
    paper.paper_id.len() == 40
        && paper.paper_id.bytes().all(|b| b.is_ascii_hexdigit())
        && paper
            .title
            .as_ref()
            .is_some_and(|title| !title.trim().is_empty())
        && paper.year.is_none_or(|year| (1..=9999).contains(&year))
}
fn exact(paper: &Paper, id: &str) -> bool {
    if id.len() == 40 {
        return paper.paper_id.eq_ignore_ascii_case(id);
    }
    if let Some(id) = id.strip_prefix("CorpusId:") {
        return paper.corpus_id.is_some_and(|value| value.to_string() == id);
    }
    let article = semantic_scholar::paper_to_article(paper);
    if let Some(value) = id.strip_prefix("DOI:") {
        let wanted = crate::discovery_merge::identifier_key(&Identifier::doi(value));
        return article
            .identifiers
            .iter()
            .any(|id| crate::discovery_merge::identifier_key(id) == wanted);
    }
    if let Some(value) = id.strip_prefix("PMID:") {
        return article.identifiers.iter().any(|id| {
            id.kind == bib_types::IdKind::Pmid && id.value.trim_start_matches('0') == value
        });
    }
    if let Some(value) = id.strip_prefix("ARXIV:") {
        return article.identifiers.iter().any(|id| {
            id.kind == bib_types::IdKind::Arxiv
                && crate::discovery_arxiv::identifier(&id.value)
                    .is_some_and(|id| crate::discovery_arxiv::version_base(&id) == value)
        });
    }
    false
}
fn checked_papers(papers: &[Paper]) -> Result<()> {
    let mut ids = HashSet::new();
    if papers
        .iter()
        .any(|paper| !valid(paper) || !ids.insert(paper.paper_id.to_ascii_lowercase()))
    {
        return Err(bad(
            "Semantic Scholar returned invalid or duplicate paper records",
        ));
    }
    Ok(())
}
pub(crate) async fn page(
    source: &crate::query::S2Source,
    plan: &SourcePlan,
    cursor: Option<&str>,
    page_size: u16,
) -> Result<SourcePage> {
    validate_page(plan, SOURCE, page_size)?;
    let client = source.client.as_ref().clone().with_max_retries(0);
    let params = &plan.parameters;
    let endpoint = params
        .get("endpoint")
        .map(String::as_str)
        .ok_or_else(|| invalid("missing Semantic Scholar endpoint"))?;
    let mut executed = params.clone();
    let mut notices = plan.notices.clone();
    if endpoint == "lookup" {
        if cursor.is_some() {
            return Err(expired(
                "Semantic Scholar singleton lookup has no continuation",
            ));
        }
        let id = params
            .get("paper_id")
            .ok_or_else(|| invalid("missing paper_id"))?;
        let articles = match client.get_paper(id, None).await {
            Ok(paper) => {
                if !valid(&paper) || !exact(&paper, id) {
                    return Err(bad(
                        "Semantic Scholar returned a different identifier or invalid singleton",
                    ));
                }
                vec![semantic_scholar::paper_to_article(&paper)]
            }
            Err(S2Error::Status { status: 404, .. }) => Vec::new(),
            Err(error) => return Err(failure(error, false)),
        };
        return Ok(SourcePage {
            source: SOURCE.into(),
            total: Some(articles.len() as u64),
            articles,
            next_cursor: None,
            executed_parameters: executed,
            query_translation: None,
            notices,
        });
    }
    let position = if let Some(cursor) = cursor {
        let ProviderCursor::SemanticScholar { position, .. } = cursor_decode(cursor, plan)? else {
            return Err(expired("invalid Semantic Scholar cursor"));
        };
        position
    } else if endpoint == "relevance" {
        Position::Relevance {
            offset: 0,
            total: 0,
        }
    } else {
        Position::Bulk {
            input_token: None,
            skip: 0,
            batch_digest: None,
            obtained: 0,
        }
    };
    let query = params.get("query").map(String::as_str).unwrap_or_default();
    let filters = filter(params)?;
    let (articles, total, next) = match (endpoint, position) {
        (
            "relevance",
            Position::Relevance {
                offset,
                total: previous_total,
            },
        ) => {
            if offset >= 1000 {
                return Err(expired(
                    "Semantic Scholar relevance window is limited to 1000 results",
                ));
            }
            let limit = u32::from(page_size).min(1000 - offset);
            let response = client
                .search_paper_filtered(query, limit, offset, &filters, None)
                .await
                .map_err(|error| failure(error, cursor.is_some()))?;
            checked_papers(&response.data)?;
            let total = response.total as u64;
            let end = offset + response.data.len() as u32;
            if response.offset != i64::from(offset)
                || response.data.len() > limit as usize
                || (offset == 0 && response.data.is_empty() && total > 0)
                || (total == 0 && !response.data.is_empty())
            {
                return Err(bad(
                    "Semantic Scholar relevance counts or offsets are inconsistent",
                ));
            }
            if cursor.is_some() && previous_total != total {
                return Err(expired(
                    "Semantic Scholar result total changed; restart relevance search",
                ));
            }
            if response
                .next
                .is_some_and(|next| next != i64::from(end) || end == offset)
            {
                return Err(bad(
                    "Semantic Scholar returned a non-progressing relevance offset",
                ));
            }
            let next = if let Some(offset) = response.next.filter(|next| *next < 1000) {
                Some(Position::Relevance {
                    offset: offset as u32,
                    total,
                })
            } else {
                if end >= 1000 && total > 1000 {
                    notices.push(SourceNotice{code:"result_window".into(),message:"Semantic Scholar exposes only the first 1000 relevance results; refine the query or choose bulk.".into()});
                } else if total > u64::from(end) {
                    notices.push(SourceNotice{code:"provider_end".into(),message:"The relevance endpoint ended continuation before its estimated total; the obtained count is not a complete corpus total.".into()});
                }
                None
            };
            executed.insert("offset".into(), offset.to_string());
            executed.insert("limit".into(), limit.to_string());
            (
                response
                    .data
                    .iter()
                    .map(semantic_scholar::paper_to_article)
                    .collect(),
                total,
                next,
            )
        }
        (
            "bulk",
            Position::Bulk {
                input_token,
                skip,
                batch_digest,
                obtained,
            },
        ) => {
            if obtained >= WINDOW {
                return Err(expired(
                    "Semantic Scholar bulk retrieval window has been reached",
                ));
            }
            if skip > 1000 || (skip > 0) != batch_digest.is_some() {
                return Err(expired("invalid Semantic Scholar batch position"));
            }
            let key = format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(plan.fingerprint.as_str(), &input_token))
                        .map_err(|_| bad("cannot bind Semantic Scholar batch"))?
                )
            );
            let batch = if let Some(batch) = source.cache.get(&key) {
                batch
            } else {
                let response = client
                    .search_paper_bulk_filtered(
                        query,
                        input_token.as_deref(),
                        None,
                        params.get("sort").map(String::as_str),
                        &filters,
                    )
                    .await
                    .map_err(|error| failure(error, cursor.is_some()))?;
                checked_papers(&response.data)?;
                if response.data.len() > 1000
                    || (response.total == 0 && !response.data.is_empty())
                    || (response.data.is_empty()
                        && (response.token.is_some() || (cursor.is_none() && response.total > 0)))
                    || (response.token.is_some() && response.token == input_token)
                {
                    return Err(bad(
                        "Semantic Scholar returned an inconsistent or non-progressing bulk batch",
                    ));
                }
                let articles = response
                    .data
                    .iter()
                    .map(semantic_scholar::paper_to_article)
                    .collect::<Vec<_>>();
                // Conversion stamps local creation times. Revalidation must
                // fingerprint upstream metadata, never those local clocks.
                let digest = serde_json::to_vec(&(&response.data, &response.token))
                    .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                    .map_err(|_| bad("cannot validate Semantic Scholar batch"))?;
                let serialized = serde_json::to_vec(&(&articles, &response.token))
                    .map_err(|_| bad("cannot validate Semantic Scholar batch"))?;
                let batch = Arc::new(Batch {
                    articles,
                    total: response.total as u64,
                    next: response.token,
                    digest,
                    bytes: serialized.len(),
                    at: Instant::now(),
                    fetched_at: chrono::Utc::now().to_rfc3339(),
                });
                source.cache.insert(key, batch.clone());
                batch
            };
            if batch_digest
                .as_ref()
                .is_some_and(|digest| digest != &batch.digest)
                || skip > batch.articles.len()
            {
                return Err(expired(
                    "Semantic Scholar buffered batch changed upstream; start a new search",
                ));
            }
            let end = (skip + usize::from(page_size))
                .min(batch.articles.len())
                .min(skip + (WINDOW - obtained) as usize);
            let articles = batch.articles[skip..end].to_vec();
            let obtained = obtained + articles.len() as u64;
            let next = if obtained >= WINDOW {
                if end < batch.articles.len() || batch.next.is_some() {
                    notices.push(SourceNotice{code:"result_window".into(),message:"Semantic Scholar bulk retrieval is limited to 10000000 records; refine the source query.".into()});
                }
                None
            } else if end < batch.articles.len() {
                Some(Position::Bulk {
                    input_token: input_token.clone(),
                    skip: end,
                    batch_digest: Some(batch.digest.clone()),
                    obtained,
                })
            } else {
                batch.next.clone().map(|token| Position::Bulk {
                    input_token: Some(token),
                    skip: 0,
                    batch_digest: None,
                    obtained,
                })
            };
            if let Some(token) = input_token {
                executed.insert("token".into(), token);
            }
            executed.insert("batch_offset".into(), skip.to_string());
            executed.insert("batch_fetched_at".into(), batch.fetched_at.clone());
            executed.insert("page_size".into(), page_size.to_string());
            (articles, batch.total, next)
        }
        _ => return Err(expired("cursor endpoint differs from the source query")),
    };
    let next_cursor = next
        .map(|position| {
            cursor_encode(ProviderCursor::SemanticScholar {
                fingerprint: plan.fingerprint.clone(),
                position,
            })
        })
        .transpose()?;
    Ok(SourcePage {
        source: SOURCE.into(),
        articles,
        total: Some(total),
        next_cursor,
        executed_parameters: executed,
        query_translation: None,
        notices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::LiteratureSource;
    fn request(query: &str) -> DiscoveryRequest {
        DiscoveryRequest {
            query: query.into(),
            sources: vec![SOURCE.into()],
            ..Default::default()
        }
    }
    fn plan(request: &DiscoveryRequest) -> Result<SourcePlan> {
        crate::discovery_plan::plan(SOURCE, request)
    }
    #[test]
    fn boolean_bulk_preserves_groups_exclusions_and_reports_non_relevance_order() {
        let result = plan(&request("(gene OR asthma) AND NOT mice")).unwrap();
        assert_eq!(result.parameters["endpoint"], "bulk");
        assert_eq!(
            result.parameters["query"],
            "(\"gene\" + -\"mice\") | (\"asthma\" + -\"mice\")"
        );
        assert_eq!(result.parameters["sort"], "paperId:asc");
        assert!(
            result
                .notices
                .iter()
                .any(|notice| notice.code == "bulk_order")
        );
        let result = plan(&request("gene AND NOT (mice OR rats)")).unwrap();
        assert!(result.parameters["query"].contains("-\"mice\""));
        assert!(result.parameters["query"].contains("-\"rats\""));
        for query in [
            "NOT mice",
            "gene OR NOT mice",
            "author:Smith",
            "gene OR year:2020",
        ] {
            assert_eq!(
                plan(&request(query)).unwrap_err().kind,
                SourceFailureKind::Unsupported,
                "{query}"
            );
        }
    }
    #[test]
    fn approximate_mode_is_explicit_and_never_removes_exclusions() {
        let mut input = request("title:\"gene editing\"");
        assert!(plan(&input).is_err());
        input.allow_approximate = true;
        let result = plan(&input).unwrap();
        assert_eq!(result.parameters["endpoint"], "relevance");
        assert_eq!(result.parameters["query"], "gene editing");
        assert!(
            result
                .notices
                .iter()
                .any(|notice| notice.code == "approximate_field_scope")
        );
        input.query = "title:gene AND NOT mice".into();
        assert_eq!(plan(&input).unwrap().parameters["endpoint"], "bulk");
        input.query = "IL-6".into();
        assert_eq!(
            plan(&input).unwrap().parameters["endpoint"],
            "bulk",
            "hyphenated terms must not use the endpoint documented to return no hyphen matches"
        );
    }
    #[test]
    fn metadata_intersections_and_type_filters_remain_constraints() {
        let mut input =
            request("(journal:Nature OR journal:Science) AND journal:Nature AND year:2020..2024");
        input.filters.publication_types = vec!["review".into(), "clinical_trial".into()];
        input.sort = DiscoverySort::Newest;
        let result = plan(&input).unwrap();
        assert_eq!(result.parameters["venue"], "Nature");
        assert_eq!(result.parameters["year"], "2020-2024");
        assert_eq!(
            result.parameters["publicationTypes"],
            "Review,ClinicalTrial"
        );
        assert_eq!(result.parameters["sort"], "publicationDate:desc");
        input.filters.open_access = Some(true);
        assert!(plan(&input).is_err());
        input.filters.open_access = None;
        input.filters.publication_types = vec!["preprint".into()];
        assert!(plan(&input).is_err());
        assert!(plan(&request("journal:Nature AND journal:Science")).is_err());
    }
    #[test]
    fn known_identifiers_are_exact_and_versioned_arxiv_is_not_substituted() {
        for (query, id) in [
            ("10.1000/Exact?literal", "DOI:10.1000/exact?literal"),
            ("pmid:00123", "PMID:123"),
            ("CorpusId:123", "CorpusId:123"),
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ] {
            let result = plan(&request(query)).unwrap();
            assert_eq!(result.parameters["endpoint"], "lookup");
            assert_eq!(result.parameters["paper_id"], id);
        }
        assert!(plan(&request("arxiv:2401.12345v2")).is_err());
        assert!(plan(&request("doi:10.1000/a AND year:2024")).is_err());
    }
    #[test]
    fn native_query_whitelist_protects_paging_credentials_and_presence_flags() {
        let mut input = DiscoveryRequest {
            sources: vec![SOURCE.into()],
            native: Some(NativeSearch {
                source: SOURCE.into(),
                parameters: [
                    ("endpoint".into(), "bulk".into()),
                    ("openAccessPdf".into(), "".into()),
                ]
                .into(),
            }),
            ..Default::default()
        };
        assert!(plan(&input).is_ok());
        for key in ["token", "offset", "limit", "api_key", "x-api-key", "fields"] {
            input
                .native
                .as_mut()
                .unwrap()
                .parameters
                .insert(key.into(), "private".into());
            assert_eq!(
                plan(&input).unwrap_err().kind,
                SourceFailureKind::InvalidQuery
            );
            input.native.as_mut().unwrap().parameters.remove(key);
        }
        input
            .native
            .as_mut()
            .unwrap()
            .parameters
            .insert("openAccessPdf".into(), "false".into());
        assert!(plan(&input).is_err());
    }
    #[test]
    fn boolean_expansion_and_batch_cache_are_bounded_and_expiry_is_not_lru_dependent() {
        let expr = SearchExpr::And {
            clauses: (0..7)
                .map(|n| SearchExpr::Or {
                    clauses: vec![
                        SearchExpr::Term {
                            value: format!("a{n}"),
                            field: None,
                        },
                        SearchExpr::Term {
                            value: format!("b{n}"),
                            field: None,
                        },
                    ],
                })
                .collect(),
        };
        let input = DiscoveryRequest {
            expression: Some(expr),
            sources: vec![SOURCE.into()],
            ..Default::default()
        };
        assert!(plan(&input).is_err());
        let cache = BulkCache::default();
        for n in 0..40 {
            cache.insert(
                n.to_string(),
                Arc::new(Batch {
                    articles: Vec::new(),
                    total: 0,
                    next: None,
                    digest: "digest".into(),
                    bytes: CACHE_BYTES / 10,
                    at: Instant::now(),
                    fetched_at: "fixture".into(),
                }),
            );
        }
        assert!(cache.get("0").is_none());
        assert!(cache.get("39").is_some());
        {
            let mut state = cache.0.lock().unwrap();
            assert!(state.bytes <= CACHE_BYTES);
            assert!(state.entries.len() <= 32);
            let (_, entry) = state.entries.back_mut().unwrap();
            Arc::get_mut(entry).unwrap().at = Instant::now() - Duration::from_secs(1801);
        }
        assert!(
            cache.get("39").is_none(),
            "an expired entry at the LRU back must not survive behind newer entries"
        );
    }

    #[tokio::test]
    async fn evicted_batches_are_refetched_and_changed_metadata_expires_continuation() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for index in 0..3 {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut head = Vec::new();
                let mut bytes = [0u8; 1024];
                while !head.windows(4).any(|v| v == b"\r\n\r\n") {
                    let count = stream.read(&mut bytes).await.unwrap();
                    assert!(count > 0);
                    head.extend_from_slice(&bytes[..count]);
                }
                let body=serde_json::json!({"total":2,"data":[{"paperId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","title":if index==2 {"Changed upstream title"}else{"First"}},{"paperId":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","title":"Second"}]}).to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
        });
        let source =
            crate::query::S2Source::new(Arc::new(S2Client::new().with_graph_base_url(endpoint)));
        let plan = source
            .plan_discovery(&DiscoveryRequest {
                page_size: 1,
                ..request("gene")
            })
            .unwrap();
        let first = page(&source, &plan, None, 1).await.unwrap();
        {
            let mut cache = source.cache.0.lock().unwrap();
            cache.entries.clear();
            cache.bytes = 0;
        }
        let second = page(&source, &plan, first.next_cursor.as_deref(), 1)
            .await
            .unwrap();
        assert_eq!(second.articles[0].title, "Second");
        assert!(second.next_cursor.is_none());
        {
            let mut cache = source.cache.0.lock().unwrap();
            cache.entries.clear();
            cache.bytes = 0;
        }
        assert_eq!(
            page(&source, &plan, first.next_cursor.as_deref(), 1)
                .await
                .unwrap_err()
                .kind,
            SourceFailureKind::CursorExpired
        );
        server.await.unwrap();
    }
}
