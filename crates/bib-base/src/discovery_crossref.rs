//! Crossref exact identifiers/filters and explicitly opted-in fuzzy text.
use crate::discovery_sources::{
    ProviderCursor, cursor_decode, cursor_encode, status_failure, validate_page,
};
use bib_types::{Identifier, SearchExpr, SearchField, discovery::*};
use std::collections::{BTreeMap, BTreeSet, HashSet};
type Result<T> = std::result::Result<T, SourceFailure>;
fn unsupported(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::Unsupported, message, false)
}
fn invalid(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::InvalidQuery, message, false)
}
fn bad_response(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::InvalidResponse, message, false)
}
fn doi(value: &str) -> Option<String> {
    crate::discovery_merge::identifier_key(&Identifier::doi(value)).map(|key| key[4..].to_owned())
}
pub(crate) fn capabilities(client: &crossref::CrossrefClient) -> SourceCapabilities {
    let mut notes=vec!["DOI and metadata filters are exact source constraints. Text requires explicit approximate matching or native query; Crossref does not enforce Boolean/phrase semantics. OA, review and clinical-trial filters are unsupported.".into(),"Cursor pages are not a stable snapshot. SDK clones share a serial connection and endpoint-specific pacing; quota is not inferred from request-rate headers.".into()];
    for limit in client.observed_rate_limits() {
        notes.push(format!(
            "Last advertised {} rate: {} requests / {} seconds; upstream concurrency {:?}.",
            limit.request_type, limit.requests, limit.interval_seconds, limit.concurrency_limit
        ));
    }
    SourceCapabilities {
        source: "crossref".into(),
        searchable: true,
        native_query: true,
        fields: vec![
            SearchField::Any,
            SearchField::Title,
            SearchField::Creator,
            SearchField::Publication,
            SearchField::Identifier,
            SearchField::Year,
        ],
        filters: vec!["year".into(), "publication_type".into()],
        sorts: vec![
            DiscoverySort::Relevance,
            DiscoverySort::Newest,
            DiscoverySort::Oldest,
            DiscoverySort::CitationCount,
        ],
        pagination: SourcePagination::Cursor,
        max_page_size: 100,
        credentials_configured: client.has_api_token(),
        authentication: SourceAuthentication::Optional,
        quota: None,
        notes,
    }
}
#[derive(Default)]
struct Clauses {
    text: BTreeMap<String, Vec<String>>,
    // Each group is a disjunction; different groups are conjunctive.
    dois: Vec<BTreeSet<String>>,
    from: Option<u16>,
    until: Option<u16>,
}
fn year(value: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .ok()
        .filter(|year| *year > 0 && *year <= 9999)
        .ok_or_else(|| invalid("invalid Crossref publication year"))
}
fn lower(target: &mut Option<u16>, value: u16) {
    *target = Some(target.unwrap_or(1).max(value));
}
fn upper(target: &mut Option<u16>, value: u16) {
    *target = Some(target.unwrap_or(9999).min(value));
}
fn text_fields(expr: &SearchExpr, fields: &mut BTreeSet<&'static str>) -> bool {
    match expr {
        SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
            clauses.iter().all(|expr| text_fields(expr, fields))
        }
        SearchExpr::Term { field, .. } | SearchExpr::Phrase { field, .. } => {
            let key = match field.unwrap_or(SearchField::Any) {
                SearchField::Any | SearchField::Title => "query.bibliographic",
                SearchField::Creator => "query.author",
                SearchField::Publication => "query.container-title",
                _ => return false,
            };
            fields.insert(key);
            true
        }
        _ => false,
    }
}
fn identifier_union(expr: &SearchExpr, ids: &mut BTreeSet<String>) -> Result<()> {
    match expr {
        SearchExpr::Or { clauses } => {
            for child in clauses {
                identifier_union(child, ids)?;
            }
        }
        SearchExpr::Term { value, .. } | SearchExpr::Phrase { value, .. } => {
            ids.insert(doi(value).ok_or_else(|| {
                unsupported("Crossref exact lookup supports DOI identifiers only")
            })?);
        }
        _ => {
            return Err(unsupported(
                "Crossref DOI disjunction must contain identifiers only",
            ));
        }
    }
    Ok(())
}
fn identifiers_only(expr: &SearchExpr) -> bool {
    match expr {
        SearchExpr::Or { clauses } => clauses.iter().all(identifiers_only),
        SearchExpr::Term {
            field: Some(SearchField::Identifier),
            ..
        }
        | SearchExpr::Phrase {
            field: Some(SearchField::Identifier),
            ..
        } => true,
        _ => false,
    }
}
fn collect(expr: &SearchExpr, clauses: &mut Clauses, approximate: bool) -> Result<()> {
    match expr {
        SearchExpr::And { clauses: children } => {
            for child in children {
                collect(child, clauses, approximate)?;
            }
            Ok(())
        }
        SearchExpr::Or { clauses: children } => {
            if identifiers_only(expr) {
                let mut ids = BTreeSet::new();
                identifier_union(expr, &mut ids)?;
                clauses.dois.push(ids);
                return Ok(());
            }
            let mut fields = BTreeSet::new();
            if !text_fields(expr, &mut fields) || fields.len() != 1 {
                return Err(unsupported(
                    "Crossref cannot preserve OR across different query fields, filters or identifiers; use another source",
                ));
            }
            for child in children {
                collect(child, clauses, approximate)?;
            }
            Ok(())
        }
        SearchExpr::Not { .. } => Err(unsupported(
            "Crossref has no strict text exclusion operator; NOT is never dropped, including in approximate mode",
        )),
        SearchExpr::Range {
            field: SearchField::Year,
            gte,
            lte,
        } => {
            if let Some(value) = gte {
                lower(&mut clauses.from, year(value)?);
            }
            if let Some(value) = lte {
                upper(&mut clauses.until, year(value)?);
            }
            Ok(())
        }
        SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field } => {
            let field = field.unwrap_or(SearchField::Any);
            match field {
                SearchField::Identifier => {
                    clauses.dois.push(
                        [doi(value).ok_or_else(|| {
                            unsupported("Crossref exact lookup supports DOI identifiers only")
                        })?]
                        .into(),
                    );
                    Ok(())
                }
                SearchField::Year => {
                    let value = year(value)?;
                    lower(&mut clauses.from, value);
                    upper(&mut clauses.until, value);
                    Ok(())
                }
                SearchField::Any
                | SearchField::Title
                | SearchField::Creator
                | SearchField::Publication => {
                    if !approximate {
                        return Err(unsupported(
                            "Crossref text is loose relevance matching; enable allow_approximate or use its native query mode",
                        ));
                    }
                    let key = match field {
                        SearchField::Any | SearchField::Title => "query.bibliographic",
                        SearchField::Creator => "query.author",
                        _ => "query.container-title",
                    };
                    clauses
                        .text
                        .entry(key.into())
                        .or_default()
                        .push(value.clone());
                    Ok(())
                }
                _ => Err(unsupported(
                    "Crossref cannot preserve this structured text field",
                )),
            }
        }
        _ => Err(unsupported(
            "Crossref supports publication-year ranges only",
        )),
    }
}
fn publication_type(value: &str) -> Result<&str> {
    match value.to_ascii_lowercase().as_str() {
        "article" | "journalarticle" | "journal_article" | "journal-article" => {
            Ok("journal-article")
        }
        "book" => Ok("book"),
        "chapter" | "book-chapter" => Ok("book-chapter"),
        "proceedings-article" => Ok("proceedings-article"),
        "posted-content" => Ok("posted-content"),
        "report" => Ok("report"),
        "dissertation" => Ok("dissertation"),
        "dataset" => Ok("dataset"),
        _ => Err(unsupported(
            "Crossref cannot map this publication type without changing its meaning; use a source-native type filter",
        )),
    }
}
pub(crate) fn parameters(request: &DiscoveryRequest) -> Result<BTreeMap<String, String>> {
    if request.filters.open_access.is_some() {
        return Err(unsupported(
            "Crossref license/link presence is not a reliable open-access predicate",
        ));
    }
    let mut clauses = Clauses::default();
    if let Some(expr) = &request.expression {
        collect(expr, &mut clauses, request.allow_approximate)?;
    }
    if let Some(value) = request.filters.year_from {
        lower(&mut clauses.from, value);
    }
    if let Some(value) = request.filters.year_to {
        upper(&mut clauses.until, value);
    }
    if matches!((clauses.from,clauses.until),(Some(from),Some(until)) if from>until) {
        return Err(unsupported(
            "Crossref cannot execute this contradictory publication-year interval",
        ));
    }
    let mut groups = clauses.dois.into_iter();
    let mut dois = groups.next().unwrap_or_default();
    let has_ids = !dois.is_empty();
    for group in groups {
        dois.retain(|id| group.contains(id));
    }
    if has_ids && dois.is_empty() {
        return Err(unsupported(
            "Crossref cannot execute contradictory DOI constraints",
        ));
    }
    if dois.len() == 1
        && clauses.text.is_empty()
        && clauses.from.is_none()
        && clauses.until.is_none()
        && request.filters.publication_types.is_empty()
    {
        return Ok([("doi".into(), dois.pop_first().unwrap())].into());
    }
    let mut filters = Vec::new();
    for id in dois {
        if id.contains(',') {
            return Err(unsupported(
                "a DOI containing a comma needs standalone exact lookup; it cannot be embedded in Crossref filter syntax",
            ));
        }
        filters.push(format!("doi:{id}"));
    }
    if let Some(value) = clauses.from {
        filters.push(format!("from-pub-date:{value:04}-01-01"));
    }
    if let Some(value) = clauses.until {
        filters.push(format!("until-pub-date:{value:04}-12-31"));
    }
    for kind in &request.filters.publication_types {
        filters.push(format!("type:{}", publication_type(kind)?));
    }
    let mut params: BTreeMap<_, _> = clauses
        .text
        .into_iter()
        .map(|(key, values)| (key, values.join(" ")))
        .collect();
    if !filters.is_empty() {
        params.insert("filter".into(), filters.join(","));
    }
    if params.is_empty() {
        return Err(invalid("Crossref requires a query, DOI or metadata filter"));
    }
    Ok(params)
}
pub(crate) fn configure_sort(
    params: &mut BTreeMap<String, String>,
    sort: DiscoverySort,
) -> Result<()> {
    if let Some(id) = params.get("doi") {
        let normalized = doi(id);
        if params.len() != 1 || normalized.is_none() {
            return Err(invalid(
                "Crossref doi lookup must be standalone; use a native DOI filter for intersections",
            ));
        }
        params.insert("doi".into(), normalized.unwrap());
        return Ok(());
    }
    let has_text = params.iter().any(|(key, value)| {
        (key == "query" || key.starts_with("query.")) && !value.trim().is_empty()
    });
    let (sort, order) = match sort {
        DiscoverySort::Relevance if has_text => ("score", "desc"),
        DiscoverySort::Relevance | DiscoverySort::Newest => ("published", "desc"),
        DiscoverySort::Oldest => ("published", "asc"),
        DiscoverySort::CitationCount => ("is-referenced-by-count", "desc"),
    };
    params.insert("sort".into(), sort.into());
    params.insert("order".into(), order.into());
    Ok(())
}
pub(crate) fn notices(params: &BTreeMap<String, String>) -> Vec<SourceNotice> {
    let mut notices=vec![SourceNotice{code:"cursor_semantics".into(),message:"Crossref cursor pages may drift as metadata changes; totals are provider-reported, not a stable snapshot.".into()}];
    if params.iter().any(|(key, value)| {
        (key == "query" || key.starts_with("query.")) && !value.trim().is_empty()
    }) {
        notices.push(SourceNotice{code:"approximate_text".into(),message:"Crossref text fields are fuzzy relevance hints. Boolean joins and exact phrases are not enforced; title hints use the broader bibliographic field. Exact identifiers and metadata filters remain constraints.".into()});
    } else if !params.contains_key("doi") {
        notices.push(SourceNotice{code:"no_text_relevance".into(),message:"Without a text query, source ordering uses the requested date/citation sort; default relevance falls back to publication date descending.".into()});
    }
    notices
}
fn failure(error: crossref::CrossrefError, continuation: bool) -> SourceFailure {
    use crossref::CrossrefError as E;
    match error {
        E::RateLimited {
            retry_after_seconds,
        } => {
            let mut error = status_failure("Crossref", 429);
            error.retry_after_seconds = retry_after_seconds;
            error
        }
        E::Status {
            status: 400 | 404 | 410,
            ..
        } if continuation => SourceFailure::new(
            SourceFailureKind::CursorExpired,
            "Crossref rejected its saved cursor; start a new search",
            false,
        ),
        E::Status { status, .. } => status_failure("Crossref", status),
        E::Http(error) => SourceFailure::new(
            if error.is_timeout() {
                SourceFailureKind::Timeout
            } else {
                SourceFailureKind::Transport
            },
            "Crossref HTTP request failed",
            true,
        ),
        E::Param(_) => invalid("Crossref rejected the query parameters or request length"),
        E::Parse(_) | E::InvalidResponse(_) => {
            bad_response("Crossref returned an invalid metadata response")
        }
        E::ResponseTooLarge => bad_response("Crossref response exceeds the supported size limit"),
    }
}
fn valid_work(work: &crossref::types::Work) -> bool {
    doi(&work.doi).is_some() && !work.title_str().trim().is_empty()
}
pub(crate) async fn page(
    client: &crossref::CrossrefClient,
    plan: &SourcePlan,
    cursor: Option<&str>,
    page_size: u16,
) -> Result<SourcePage> {
    validate_page(plan, "crossref", page_size)?;
    if let Some(id) = plan.parameters.get("doi") {
        if cursor.is_some() {
            return Err(SourceFailure::new(
                SourceFailureKind::CursorExpired,
                "standalone Crossref DOI lookup has no continuation",
                false,
            ));
        }
        let articles = match client.works_by_doi(id).await {
            Ok(response) => {
                let work = response
                    .message
                    .filter(|work| {
                        response.message_type == "work"
                            && valid_work(work)
                            && doi(&work.doi) == doi(id)
                    })
                    .ok_or_else(|| {
                        bad_response(
                            "Crossref returned a different DOI or invalid singleton record",
                        )
                    })?;
                vec![crossref::work_to_article(&work)]
            }
            Err(crossref::CrossrefError::Status { status: 404, .. }) => Vec::new(),
            Err(error) => return Err(failure(error, false)),
        };
        return Ok(SourcePage {
            source: "crossref".into(),
            total: Some(articles.len() as u64),
            articles,
            next_cursor: None,
            executed_parameters: plan.parameters.clone(),
            query_translation: None,
            notices: plan.notices.clone(),
        });
    }
    let (raw_cursor, obtained) = if let Some(cursor) = cursor {
        let ProviderCursor::Crossref {
            cursor, obtained, ..
        } = cursor_decode(cursor, plan)?
        else {
            return Err(SourceFailure::new(
                SourceFailureKind::CursorExpired,
                "invalid Crossref cursor",
                false,
            ));
        };
        (cursor, obtained)
    } else {
        ("*".into(), 0)
    };
    let mut query = crossref::client::WorksQuery::new()
        .with_rows(u32::from(page_size))
        .with_cursor(&raw_cursor);
    for (key, value) in &plan.parameters {
        query = match key.as_str() {
            "query" => query.with_query(value),
            "sort" => query.with_sort(value),
            "order" => query.with_order(value),
            "filter" => {
                let mut query = query;
                for filter in value.split(',') {
                    let (key, value) = filter
                        .split_once(':')
                        .ok_or_else(|| invalid("invalid Crossref filter syntax"))?;
                    query = query.with_filter(key, value);
                }
                query
            }
            _ if key.starts_with("query.") => query.with_field_query(key, value),
            _ => return Err(invalid("unrecognized Crossref source plan parameter")),
        };
    }
    let response = client
        .works(&query)
        .await
        .map_err(|error| failure(error, cursor.is_some()))?;
    let message = response.message;
    let count = message.items.len();
    let mut seen = HashSet::new();
    let expected_dois: HashSet<_> = query
        .filters
        .iter()
        .filter(|(key, _)| key == "doi")
        .filter_map(|(_, value)| doi(value))
        .collect();
    if count > usize::from(page_size)
        || message
            .items_per_page
            .is_some_and(|size| size > u32::from(page_size))
        || (message.total_results == 0 && count > 0)
        || (cursor.is_none() && count == 0 && message.total_results > 0)
        || message.items.iter().any(|work| {
            !valid_work(work)
                || !seen.insert(doi(&work.doi).unwrap())
                || (!expected_dois.is_empty() && !expected_dois.contains(&doi(&work.doi).unwrap()))
        })
    {
        return Err(bad_response(
            "Crossref returned inconsistent counts, identifiers or records",
        ));
    }
    let end = obtained.saturating_add(count as u64);
    let has_more = count == usize::from(page_size) && end < message.total_results;
    let next_cursor = if has_more {
        let raw = message
            .next_cursor
            .filter(|cursor| !cursor.is_empty())
            .ok_or_else(|| bad_response("Crossref omitted continuation for a full page"))?;
        Some(cursor_encode(ProviderCursor::Crossref {
            fingerprint: plan.fingerprint.clone(),
            cursor: raw,
            obtained: end,
        })?)
    } else {
        None
    };
    let mut executed = plan.parameters.clone();
    executed.insert("cursor".into(), raw_cursor);
    executed.insert("rows".into(), page_size.to_string());
    Ok(SourcePage {
        source: "crossref".into(),
        articles: message
            .items
            .iter()
            .map(crossref::work_to_article)
            .collect(),
        total: Some(message.total_results),
        next_cursor,
        executed_parameters: executed,
        query_translation: None,
        notices: plan.notices.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(query: &str) -> DiscoveryRequest {
        DiscoveryRequest {
            query: query.into(),
            sources: vec!["crossref".into()],
            ..Default::default()
        }
    }
    fn plan(request: &DiscoveryRequest) -> Result<SourcePlan> {
        crate::discovery_plan::plan("crossref", request)
    }
    #[test]
    fn strict_ids_and_filters_do_not_require_approximate_matching() {
        assert_eq!(
            plan(&request("https://doi.org/10.1000/Exact?literal"))
                .unwrap()
                .parameters["doi"],
            "10.1000/exact?literal"
        );
        let mut input = request("year:2020..2024");
        input.filters.publication_types = vec!["article".into(), "book".into()];
        let result = plan(&input).unwrap();
        assert_eq!(
            result.parameters["filter"],
            "from-pub-date:2020-01-01,until-pub-date:2024-12-31,type:journal-article,type:book"
        );
        assert_eq!(result.parameters["sort"], "published");
        let result = plan(&request("doi:10.1000/a OR doi:10.1000/b")).unwrap();
        assert_eq!(result.parameters["filter"], "doi:10.1000/a,doi:10.1000/b");
        assert!(plan(&request("doi:10.1000/a AND doi:10.1000/b")).is_err());
    }
    #[test]
    fn fuzzy_text_needs_opt_in_and_exclusions_are_never_dropped() {
        let mut input = request("title:\"gene editing\" AND author:Smith");
        assert_eq!(
            plan(&input).unwrap_err().kind,
            SourceFailureKind::Unsupported
        );
        input.allow_approximate = true;
        let result = plan(&input).unwrap();
        assert_eq!(result.parameters["query.bibliographic"], "gene editing");
        assert_eq!(result.parameters["query.author"], "Smith");
        assert!(
            result
                .notices
                .iter()
                .any(|notice| notice.code == "approximate_text")
        );
        input.query = "gene AND NOT mice".into();
        assert_eq!(
            plan(&input).unwrap_err().kind,
            SourceFailureKind::Unsupported
        );
        input.query = "abstract:gene".into();
        assert!(plan(&input).is_err());
        input.query = "gene".into();
        input.filters.open_access = Some(true);
        assert!(plan(&input).is_err());
        input.filters.open_access = None;
        input.filters.publication_types = vec!["review".into()];
        assert!(plan(&input).is_err());
    }
    #[test]
    fn doi_intersections_preserve_boolean_groups_and_other_filters() {
        let result = plan(&request("(doi:10.1000/a OR doi:10.1000/b) AND year:2020")).unwrap();
        assert_eq!(
            result.parameters["filter"],
            "doi:10.1000/a,doi:10.1000/b,from-pub-date:2020-01-01,until-pub-date:2020-12-31"
        );
        let result = plan(&request(
            "(doi:10.1000/a OR doi:10.1000/b) AND doi:10.1000/b",
        ))
        .unwrap();
        assert_eq!(result.parameters["doi"], "10.1000/b");
        assert!(
            plan(&request(
                "(doi:10.1000/a OR doi:10.1000/b) AND doi:10.1000/c"
            ))
            .is_err()
        );
        assert!(plan(&request("doi:10.1000/a OR year:2020")).is_err());
    }
    #[test]
    fn approximate_or_never_becomes_an_intersection_of_source_fields() {
        let mut input = request("title:gene OR author:Smith");
        input.allow_approximate = true;
        assert_eq!(
            plan(&input).unwrap_err().kind,
            SourceFailureKind::Unsupported
        );
        input.query = "title:gene OR title:asthma".into();
        assert_eq!(
            plan(&input).unwrap().parameters["query.bibliographic"],
            "gene asthma"
        );
    }
    #[test]
    fn empty_native_text_does_not_claim_relevance() {
        let mut params = [
            ("query".into(), "  ".into()),
            ("filter".into(), "type:book".into()),
        ]
        .into();
        configure_sort(&mut params, DiscoverySort::Relevance).unwrap();
        assert_eq!(params["sort"], "published");
        assert!(
            !notices(&params)
                .iter()
                .any(|notice| notice.code == "approximate_text")
        );
        assert!(
            notices(&params)
                .iter()
                .any(|notice| notice.code == "no_text_relevance")
        );
    }
    #[test]
    fn native_mode_rejects_credentials_paging_and_deprecated_title() {
        let mut input = DiscoveryRequest {
            sources: vec!["crossref".into()],
            native: Some(NativeSearch {
                source: "crossref".into(),
                parameters: [("query.bibliographic".into(), "gene editing".into())].into(),
            }),
            ..Default::default()
        };
        assert!(plan(&input).is_ok());
        for key in ["mailto", "cursor", "rows", "query.title", "api_key"] {
            input
                .native
                .as_mut()
                .unwrap()
                .parameters
                .insert(key.into(), "secret".into());
            assert_eq!(
                plan(&input).unwrap_err().kind,
                SourceFailureKind::InvalidQuery
            );
            input.native.as_mut().unwrap().parameters.remove(key);
        }
    }
}
