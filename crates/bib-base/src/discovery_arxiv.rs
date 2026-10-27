//! arXiv's offset API and field grammar. Publication-year and OA filters are
//! deliberately not translated into different submission/license semantics.
use crate::discovery_sources::{
    ProviderCursor, cursor_decode, cursor_encode, status_failure, validate_page,
};
use bib_types::{SearchExpr, SearchField, discovery::*};
use std::collections::{BTreeMap, HashSet};
type Result<T> = std::result::Result<T, SourceFailure>;
fn unsupported(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::Unsupported, message, false)
}
fn invalid_response(message: &str) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::InvalidResponse, message, false)
}

/// Preserve explicit versions; URL wrappers never become query operators.
pub(crate) fn identifier(value: &str) -> Option<String> {
    let mut value = value.trim().to_ascii_lowercase();
    for prefix in [
        "https://arxiv.org/abs/",
        "http://arxiv.org/abs/",
        "https://arxiv.org/pdf/",
        "http://arxiv.org/pdf/",
        "arxiv:",
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = if prefix.starts_with("http") {
                rest.split(['?', '#'])
                    .next()?
                    .trim_end_matches('/')
                    .trim_end_matches(".pdf")
                    .to_owned()
            } else {
                rest.trim().to_owned()
            };
            break;
        }
    }
    if value.len() > 100 {
        return None;
    }
    let base = if value.rsplit('/').next()?.contains('v') {
        let (base, version) = value.rsplit_once('v')?;
        if version
            .parse::<u32>()
            .ok()
            .is_none_or(|version| version == 0)
        {
            return None;
        }
        base
    } else {
        &value
    };
    let valid_month = |digits: &str| {
        digits.len() >= 4
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && digits[2..4]
                .parse::<u8>()
                .is_ok_and(|month| (1..=12).contains(&month))
    };
    let valid = if let Some((archive, number)) = base.split_once('/') {
        archive
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && archive
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
            && number.len() == 7
            && valid_month(number)
    } else if let Some((date, number)) = base.split_once('.') {
        date.len() == 4
            && valid_month(date)
            && (4..=5).contains(&number.len())
            && number.bytes().all(|byte| byte.is_ascii_digit())
    } else {
        false
    };
    valid.then_some(value)
}
pub(crate) fn capabilities() -> SourceCapabilities {
    SourceCapabilities { source: "arxiv".into(), searchable: true, native_query: true, fields: vec![SearchField::Any,SearchField::Title,SearchField::Abstract,SearchField::Creator,SearchField::Publication,SearchField::Identifier], filters: Vec::new(), sorts: vec![DiscoverySort::Relevance,DiscoverySort::Newest,DiscoverySort::Oldest], pagination: SourcePagination::Offset, max_page_size:100, credentials_configured:false, authentication:SourceAuthentication::None, quota:None, notes:vec!["One connection and at least three seconds between request starts, shared with SDK fetches. At most 30000 results are retrievable; offset paging is not a snapshot.".into(),"Publication-year, publication-type and open-access filters are unsupported. Native submittedDate queries filter first-submission dates. NOT needs a positive AND anchor.".into()] }
}
fn flatten_and<'a>(expr: &'a SearchExpr, clauses: &mut Vec<&'a SearchExpr>) {
    let expr = if let SearchExpr::Not { clause } = expr {
        if let SearchExpr::Not { clause } = clause.as_ref() {
            clause.as_ref()
        } else {
            expr
        }
    } else {
        expr
    };
    if let SearchExpr::And { clauses: children } = expr {
        for child in children {
            flatten_and(child, clauses);
        }
    } else {
        clauses.push(expr);
    }
}
fn expression(expr: &SearchExpr) -> Result<String> {
    match expr {
        SearchExpr::And { .. } => {
            let mut clauses = Vec::new();
            flatten_and(expr, &mut clauses);
            let (negative, positive): (Vec<_>, Vec<_>) = clauses
                .into_iter()
                .partition(|clause| matches!(clause, SearchExpr::Not { .. }));
            if positive.is_empty() {
                return Err(unsupported(
                    "arXiv requires a positive condition before NOT; use native search_query for other source syntax",
                ));
            }
            let mut rendered = positive
                .into_iter()
                .map(expression)
                .collect::<Result<Vec<_>>>()?
                .join(" AND ");
            for clause in negative {
                if let SearchExpr::Not { clause } = clause {
                    rendered = format!("({rendered}) ANDNOT {}", expression(clause)?);
                }
            }
            Ok(format!("({rendered})"))
        }
        SearchExpr::Or { clauses } => Ok(format!(
            "({})",
            clauses
                .iter()
                .map(expression)
                .collect::<Result<Vec<_>>>()?
                .join(" OR ")
        )),
        SearchExpr::Not { clause } => {
            if let SearchExpr::Not { clause } = clause.as_ref() {
                expression(clause)
            } else {
                Err(unsupported(
                    "this Boolean shape is not supported by the arXiv translator; use direct AND exclusions or native search_query",
                ))
            }
        }
        SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field } => {
            let tag = match field.unwrap_or(SearchField::Any) {
                SearchField::Any => "all",
                SearchField::Title => "ti",
                SearchField::Abstract => "abs",
                SearchField::Creator => "au",
                SearchField::Publication => "jr",
                SearchField::Year => {
                    return Err(unsupported(
                        "arXiv supports submission dates, not publication-year filters; use native submittedDate",
                    ));
                }
                SearchField::Identifier => {
                    return Err(unsupported(
                        "an arXiv identifier must be the sole structured condition; use native id_list with search_query for intersections",
                    ));
                }
                _ => {
                    return Err(unsupported(
                        "arXiv does not support this structured search field",
                    ));
                }
            };
            Ok(format!("{tag}:{}", crate::discovery_plan::literal(value)?))
        }
        _ => Err(unsupported(
            "arXiv supports submission-date ranges only in native search_query",
        )),
    }
}
pub(crate) fn parameters(request: &DiscoveryRequest) -> Result<BTreeMap<String, String>> {
    if request.filters != DiscoveryFilters::default() {
        return Err(unsupported(
            "arXiv cannot apply publication-year, publication-type or open-access filters; use native submittedDate for submission dates",
        ));
    }
    let expr = request
        .expression
        .as_ref()
        .ok_or_else(|| unsupported("arXiv requires a supported query"))?;
    if let SearchExpr::Term {
        value,
        field: Some(SearchField::Identifier),
    }
    | SearchExpr::Phrase {
        value,
        field: Some(SearchField::Identifier),
    } = expr
    {
        let id = identifier(value).ok_or_else(|| {
            unsupported("arXiv exact lookup supports arXiv identifiers, not DOI or PMID")
        })?;
        return Ok([("id_list".into(), id)].into());
    }
    Ok([("search_query".into(), expression(expr)?)].into())
}
pub(crate) fn validate_identifiers(parameters: &BTreeMap<String, String>) -> Result<()> {
    if let Some(ids) = parameters.get("id_list") {
        if ids.split(',').any(|id| identifier(id).is_none()) {
            return Err(SourceFailure::new(
                SourceFailureKind::InvalidQuery,
                "arXiv id_list must contain valid comma-separated arXiv identifiers",
                false,
            ));
        }
    }
    Ok(())
}
pub(crate) fn version_base(id: &str) -> &str {
    if id.rsplit('/').next().is_some_and(|part| part.contains('v')) {
        id.rsplit_once('v').map_or(id, |(base, _)| base)
    } else {
        id
    }
}
fn error(error: arxiv::error::ArxivError) -> SourceFailure {
    use arxiv::error::ArxivError as E;
    match error {
        E::RateLimited {
            retry_after_seconds,
        } => {
            let mut failure = status_failure("arXiv", 429);
            failure.retry_after_seconds = retry_after_seconds;
            failure
        }
        E::Status { status, .. } => status_failure("arXiv", status),
        E::Http(error) => SourceFailure::new(
            if error.is_timeout() {
                SourceFailureKind::Timeout
            } else {
                SourceFailureKind::Transport
            },
            "arXiv HTTP request failed",
            true,
        ),
        E::Param(_) | E::Api(_) => SourceFailure::new(
            SourceFailureKind::InvalidQuery,
            "arXiv rejected the source query or identifier",
            false,
        ),
        E::Xml(_) => invalid_response("arXiv returned an invalid Atom response"),
        E::ResponseTooLarge => invalid_response("arXiv response exceeds the supported size limit"),
    }
}
pub(crate) async fn page(
    client: &arxiv::ArxivClient,
    plan: &SourcePlan,
    cursor: Option<&str>,
    page_size: u16,
) -> Result<SourcePage> {
    validate_page(plan, "arxiv", page_size)?;
    let (offset, previous_total) = if let Some(cursor) = cursor {
        let ProviderCursor::Arxiv { offset, total, .. } = cursor_decode(cursor, plan)? else {
            return Err(SourceFailure::new(
                SourceFailureKind::CursorExpired,
                "invalid arXiv cursor",
                false,
            ));
        };
        if offset >= 30000 || u64::from(offset) >= total {
            return Err(SourceFailure::new(
                SourceFailureKind::CursorExpired,
                "arXiv cursor is outside the retrievable window",
                false,
            ));
        }
        (offset, Some(total))
    } else {
        (0, None)
    };
    let mut request = arxiv::types::SearchRequest::new(
        plan.parameters
            .get("search_query")
            .cloned()
            .unwrap_or_default(),
    )
    .start(offset)
    .max_results(u32::from(page_size).min(30000 - offset));
    if let Some(ids) = plan.parameters.get("id_list") {
        request = request.id_list(ids);
    }
    request = request
        .sort_by(match plan.parameters.get("sortBy").map(String::as_str) {
            Some("submittedDate") => arxiv::types::SortBy::SubmittedDate,
            _ => arxiv::types::SortBy::Relevance,
        })
        .sort_order(
            if plan
                .parameters
                .get("sortOrder")
                .is_some_and(|value| value == "ascending")
            {
                arxiv::types::SortOrder::Ascending
            } else {
                arxiv::types::SortOrder::Descending
            },
        );
    let response = client.search(&request).await.map_err(error)?;
    if previous_total.is_some_and(|total| total != response.total_results) {
        return Err(SourceFailure::new(
            SourceFailureKind::CursorExpired,
            "arXiv result count changed; start a new search to avoid inconsistent offset pages",
            false,
        ));
    }
    let count = response.entries.len() as u64;
    let requested_ids: Vec<_> = plan
        .parameters
        .get("id_list")
        .into_iter()
        .flat_map(|ids| ids.split(','))
        .filter_map(identifier)
        .collect();
    if !requested_ids.is_empty()
        && response.entries.iter().any(|entry| {
            let Some(id) = identifier(&entry.arxiv_id) else {
                return true;
            };
            !requested_ids.iter().any(|requested| {
                requested == &id
                    || (version_base(requested) == requested && version_base(&id) == requested)
            })
        })
    {
        return Err(invalid_response(
            "arXiv returned a different identifier or version from the requested id_list",
        ));
    }
    let mut ids = HashSet::new();
    if response.start_index != u64::from(offset)
        || count > u64::from(page_size)
        || response.items_per_page > u64::from(page_size)
        || (count > 0 && response.items_per_page == 0)
        || u64::from(offset) + count > response.total_results
        || (count == 0 && response.total_results > u64::from(offset))
        || response.entries.iter().any(|entry| {
            entry.title.trim().is_empty()
                || identifier(&entry.arxiv_id).is_none()
                || !ids.insert(entry.arxiv_id.clone())
        })
    {
        return Err(invalid_response(
            "arXiv returned inconsistent pagination or invalid records",
        ));
    }
    let end = u64::from(offset) + count;
    let next_cursor = if end < response.total_results.min(30000) {
        Some(cursor_encode(ProviderCursor::Arxiv {
            fingerprint: plan.fingerprint.clone(),
            offset: end as u32,
            total: response.total_results,
        })?)
    } else {
        None
    };
    let mut notices = plan.notices.clone();
    notices.push(SourceNotice { code:"offset_paging".into(),message:"arXiv offset results may shift upstream; a changed total invalidates continuation. Counts refer to the source query, not a stable snapshot.".into() });
    if response.total_results > 30000 {
        notices.push(SourceNotice { code:"source_window_limit".into(),message:"arXiv exposes at most the first 30000 results; narrow this query to retrieve the remainder.".into() });
    }
    let mut executed = plan.parameters.clone();
    executed.insert("start".into(), offset.to_string());
    executed.insert(
        "max_results".into(),
        request.max_results.unwrap().to_string(),
    );
    Ok(SourcePage {
        source: "arxiv".into(),
        articles: arxiv::atom_to_articles(&response.entries),
        total: Some(response.total_results),
        next_cursor,
        executed_parameters: executed,
        query_translation: None,
        notices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan(query: &str) -> Result<SourcePlan> {
        crate::discovery_plan::plan(
            "arxiv",
            &DiscoveryRequest {
                query: query.into(),
                sources: vec!["arxiv".into()],
                ..Default::default()
            },
        )
    }
    #[test]
    fn structured_fields_groups_and_anchored_negation_preserve_semantics() {
        let result =
            plan("(title:gene OR abstract:\"gene editing\") AND NOT author:Smith").unwrap();
        assert_eq!(
            result.parameters["search_query"],
            "(((ti:\"gene\" OR abs:\"gene editing\")) ANDNOT au:\"Smith\")"
        );
        assert_eq!(
            plan("gene AND NOT NOT mice").unwrap().parameters["search_query"],
            "(all:\"gene\" AND all:\"mice\")"
        );
        for query in [
            "NOT gene",
            "gene OR NOT mice",
            "year:2024",
            "fulltext:gene",
            "doi:10.1000/test",
        ] {
            assert_eq!(
                plan(query).unwrap_err().kind,
                SourceFailureKind::Unsupported,
                "{query}"
            );
        }
        let mut request = DiscoveryRequest {
            query: "gene".into(),
            sources: vec!["arxiv".into()],
            ..Default::default()
        };
        request.filters.year_from = Some(2020);
        assert_eq!(
            crate::discovery_plan::plan("arxiv", &request)
                .unwrap_err()
                .kind,
            SourceFailureKind::Unsupported
        );
    }
    #[test]
    fn exact_identifiers_preserve_versions_and_use_id_list() {
        for value in [
            "2401.12345v2",
            "arXiv:2401.12345v2",
            "HTTPS://ARXIV.ORG/abs/2401.12345v2",
            "https://arxiv.org/pdf/2401.12345v2.pdf",
        ] {
            assert_eq!(plan(value).unwrap().parameters["id_list"], "2401.12345v2");
        }
        assert_eq!(
            plan("arxiv:hep-th/9901001v3").unwrap().parameters["id_list"],
            "hep-th/9901001v3"
        );
        assert_eq!(
            identifier("solv-int/9701001").as_deref(),
            Some("solv-int/9701001")
        );
        assert_eq!(
            identifier("math.GT/0309136").as_deref(),
            Some("math.gt/0309136")
        );
        for value in [
            "2400.12345",
            "2413.12345",
            "2401.12",
            "2401.12345v0",
            "2401.12345v",
            "evil/2401001 OR all:*",
        ] {
            assert!(identifier(value).is_none(), "{value}");
        }
    }
    #[test]
    fn native_submission_dates_are_explicit_and_paging_is_not_user_controlled() {
        let mut request = DiscoveryRequest {
            sources: vec!["arxiv".into()],
            native: Some(NativeSearch {
                source: "arxiv".into(),
                parameters: [(
                    "search_query".into(),
                    "submittedDate:[202301010000 TO 202412312359]".into(),
                )]
                .into(),
            }),
            ..Default::default()
        };
        assert!(
            crate::discovery_plan::plan("arxiv", &request)
                .unwrap()
                .parameters["search_query"]
                .starts_with("submittedDate:")
        );
        request
            .native
            .as_mut()
            .unwrap()
            .parameters
            .insert("start".into(), "100".into());
        assert_eq!(
            crate::discovery_plan::plan("arxiv", &request)
                .unwrap_err()
                .kind,
            SourceFailureKind::InvalidQuery
        );
        request.native.as_mut().unwrap().parameters.remove("start");
        request
            .native
            .as_mut()
            .unwrap()
            .parameters
            .insert("id_list".into(), "2401.12345v0".into());
        assert_eq!(
            crate::discovery_plan::plan("arxiv", &request)
                .unwrap_err()
                .kind,
            SourceFailureKind::InvalidQuery
        );
    }
}
