//! Pure source-aware planning; unsupported semantics never become a wider query.
use bib_types::{SearchExpr, SearchField, discovery::*};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type Planned<T> = std::result::Result<T, SourceFailure>;
fn invalid(message: impl Into<String>) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::InvalidQuery, message, false)
}
fn unsupported(message: impl Into<String>) -> SourceFailure {
    SourceFailure::new(SourceFailureKind::Unsupported, message, false)
}

pub fn normalize(mut request: DiscoveryRequest) -> Planned<DiscoveryRequest> {
    if request.query.len() > 16_384 {
        return Err(invalid("query exceeds 16384 bytes"));
    }
    if !(1..=100).contains(&request.page_size) {
        return Err(invalid("page_size must be between 1 and 100"));
    }
    if !(100..=120_000).contains(&request.source_timeout_ms)
        || !(100..=120_000).contains(&request.deadline_ms)
    {
        return Err(invalid("timeouts must be between 100 and 120000 ms"));
    }
    let modes = usize::from(!request.query.trim().is_empty())
        + usize::from(request.expression.is_some())
        + usize::from(request.native.is_some());
    if modes > 1 {
        return Err(invalid("supply only one of query, expression, or native"));
    }
    if request.sources.is_empty() || request.sources.len() > 8 {
        return Err(invalid("choose between 1 and 8 sources"));
    }
    let mut unique = std::collections::HashSet::new();
    if request.sources.iter().any(|source| {
        source.is_empty()
            || source.len() > 64
            || !source
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit())
            || !unique.insert(source.clone())
    }) {
        return Err(invalid("sources must be distinct lowercase source names"));
    }
    if let Some(native) = &request.native {
        if request.sources != [native.source.clone()] {
            return Err(invalid("native mode must select exactly its own source"));
        }
        if request.filters != DiscoveryFilters::default() {
            return Err(invalid(
                "native mode takes filters in its source parameters",
            ));
        }
        if native.parameters.is_empty()
            || native.parameters.len() > 8
            || native
                .parameters
                .values()
                .all(|value| value.trim().is_empty())
            || native
                .parameters
                .values()
                .any(|v| v.len() > 16_384 || v.chars().any(char::is_control))
        {
            return Err(invalid("invalid native parameters"));
        }
    }
    let f = &request.filters;
    if f.year_from.is_some_and(|y| y == 0 || y > 9999)
        || f.year_to.is_some_and(|y| y == 0 || y > 9999)
        || matches!((f.year_from,f.year_to),(Some(a),Some(b)) if a>b)
    {
        return Err(invalid("invalid publication year bounds"));
    }
    if f.publication_types.len() > 50
        || f.publication_types
            .iter()
            .any(|v| v.trim().is_empty() || v.len() > 120 || v.chars().any(char::is_control))
    {
        return Err(invalid("invalid publication types"));
    }
    if !request.query.trim().is_empty() {
        let mut doi = request.query.trim().to_owned();
        for prefix in [
            "https://doi.org/",
            "http://doi.org/",
            "https://dx.doi.org/",
            "http://dx.doi.org/",
            "doi:",
        ] {
            if doi
                .get(..prefix.len())
                .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
            {
                doi = doi[prefix.len()..].trim().to_owned();
                break;
            }
        }
        let is_doi = doi.split_once('/').is_some_and(|(prefix, suffix)| {
            prefix.strip_prefix("10.").is_some_and(|registrant| {
                (4..=9).contains(&registrant.len())
                    && registrant.bytes().all(|b| b.is_ascii_digit())
            }) && !suffix.is_empty()
                && !doi.chars().any(char::is_whitespace)
        });
        let source_id = request.query.trim();
        let s2_id = request
            .sources
            .iter()
            .any(|source| source == "semantic_scholar")
            && ((source_id.len() == 40 && source_id.bytes().all(|b| b.is_ascii_hexdigit()))
                || source_id.split_once(':').is_some_and(|(prefix, _)| {
                    prefix.eq_ignore_ascii_case("s2") || prefix.eq_ignore_ascii_case("corpusid")
                }));
        request.expression = if s2_id {
            Some(SearchExpr::Term {
                value: crate::discovery_s2::identifier(source_id)?,
                field: Some(SearchField::Identifier),
            })
        } else if let Some(id) = crate::discovery_arxiv::identifier(&request.query) {
            Some(SearchExpr::Term {
                value: id,
                field: Some(SearchField::Identifier),
            })
        } else if is_doi {
            Some(SearchExpr::Term {
                value: doi,
                field: Some(SearchField::Identifier),
            })
        } else {
            bib_types::parse_search_query(&request.query).map_err(|e| invalid(e.to_string()))?
        };
        request.query.clear();
    }
    if let Some(expr) = &request.expression {
        crate::local_index::validate(expr, 0, &mut 0).map_err(|e| invalid(e.to_string()))?;
    }
    if request.expression.is_none()
        && request.native.is_none()
        && request.filters == DiscoveryFilters::default()
    {
        return Err(invalid("provide a query or discovery filters"));
    }
    Ok(request)
}

/// Called with a normalized request, and safe to call directly as well.
pub fn plan(source: &str, request: &DiscoveryRequest) -> Planned<SourcePlan> {
    let request = normalize(request.clone())?;
    if !request.sources.iter().any(|s| s == source) {
        return Err(invalid("source is outside this request"));
    }
    let mut params = if let Some(native) = &request.native {
        let allowed: &[&str] = match source {
            "pubmed" => &["term"],
            "arxiv" => &["search_query", "id_list"],
            "crossref" => &[
                "doi",
                "query",
                "query.bibliographic",
                "query.author",
                "query.container-title",
                "query.affiliation",
                "filter",
            ],
            "semantic_scholar" => &[
                "endpoint",
                "query",
                "paper_id",
                "year",
                "publicationDateOrYear",
                "venue",
                "publicationTypes",
                "fieldsOfStudy",
                "openAccessPdf",
                "minCitationCount",
            ],
            "openalex" => &[
                "search",
                "search.exact",
                "search.title",
                "search.title.exact",
                "search.title_and_abstract",
                "search.title_and_abstract.exact",
                "filter",
            ],
            _ => {
                return Err(unsupported(format!(
                    "{source} native search is not supported"
                )));
            }
        };
        if native
            .parameters
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
        {
            return Err(invalid(
                "native parameters include an unsupported key (credentials and paging are not accepted)",
            ));
        }
        native.parameters.clone()
    } else {
        match source {
            "pubmed" => pubmed(&request)?,
            "arxiv" => crate::discovery_arxiv::parameters(&request)?,
            "crossref" => crate::discovery_crossref::parameters(&request)?,
            "semantic_scholar" => crate::discovery_s2::parameters(&request)?,
            "openalex" => openalex(&request)?,
            _ => {
                return Err(unsupported(format!(
                    "{source} does not support discovery sessions yet"
                )));
            }
        }
    };
    match (source, request.sort) {
        ("crossref", sort) => crate::discovery_crossref::configure_sort(&mut params, sort)?,
        ("semantic_scholar", sort) => crate::discovery_s2::configure(&mut params, sort)?,
        ("pubmed", DiscoverySort::Relevance) => {
            params.insert("sort".into(), "relevance".into());
        }
        ("pubmed", DiscoverySort::Newest) => {
            params.insert("sort".into(), "pub_date".into());
        }
        ("pubmed", _) => return Err(unsupported("PubMed does not support this sort")),
        ("openalex", DiscoverySort::Relevance) => {}
        ("openalex", DiscoverySort::Newest) => {
            params.insert("sort".into(), "publication_date:desc".into());
        }
        ("openalex", DiscoverySort::Oldest) => {
            params.insert("sort".into(), "publication_date:asc".into());
        }
        ("openalex", DiscoverySort::CitationCount) => {
            params.insert("sort".into(), "cited_by_count:desc".into());
        }
        ("arxiv", sort) => {
            crate::discovery_arxiv::validate_identifiers(&params)?;
            let (by, direction) = match sort {
                DiscoverySort::Relevance => ("relevance", "descending"),
                DiscoverySort::Newest => ("submittedDate", "descending"),
                DiscoverySort::Oldest => ("submittedDate", "ascending"),
                _ => return Err(unsupported("arXiv does not support citation-count sorting")),
            };
            if params.values().all(|value| value.trim().is_empty()) {
                return Err(invalid("arXiv requires search_query or id_list"));
            }
            params.insert("sortBy".into(), by.into());
            params.insert("sortOrder".into(), direction.into());
        }
        _ => return Err(unsupported("unknown discovery source")),
    }
    let mut hash = Sha256::new();
    hash.update(source);
    hash.update(serde_json::to_vec(&params).map_err(|e| invalid(e.to_string()))?);
    hash.update(request.page_size.to_le_bytes());
    let mut notices=vec![SourceNotice{code:"source_semantics".into(),message:if request.native.is_some(){"Native parameters are forwarded unchanged; matching and expansion follow the selected source."}else{match source {"pubmed"=>"Unfielded structured terms search title/abstract; PubMed query translation is returned with each page.","arxiv"=>"arXiv uses its native field matching. Newest/oldest refer to first submission date, not journal publication date. Offset pages are not an upstream snapshot.","crossref"=>"Crossref identifiers and filters use provider constraints; bibliographic text matching is loose relevance search, not strict Boolean or phrase matching.",_=>"OpenAlex exact text mode avoids stemming; field coverage and phrase handling follow the upstream search engine."}}.into()}];
    if source == "crossref" {
        notices.extend(crate::discovery_crossref::notices(&params));
    }
    if source == "semantic_scholar" {
        notices = crate::discovery_s2::notices(&params);
        notices.extend(crate::discovery_s2::field_notices(&request));
    }
    Ok(SourcePlan {
        source: source.into(),
        page_size: request.page_size,
        parameters: params,
        fingerprint: format!("{:x}", hash.finalize()),
        notices,
    })
}

pub(crate) fn literal(value: &str) -> Planned<String> {
    if value.trim().is_empty()
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\\' | '[' | ']' | '*' | '?' | '~'))
    {
        return Err(unsupported(
            "literal contains source query syntax; use native mode for wildcards, fuzzy, or special syntax",
        ));
    }
    Ok(format!("\"{value}\""))
}
fn year(value: &str) -> Planned<u16> {
    value
        .parse::<u16>()
        .ok()
        .filter(|y| *y > 0 && *y <= 9999)
        .ok_or_else(|| invalid("invalid year"))
}
fn pubmed_expr(expr: &SearchExpr) -> Planned<String> {
    match expr {
        SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
            let op = if matches!(expr, SearchExpr::And { .. }) {
                " AND "
            } else {
                " OR "
            };
            Ok(format!(
                "({})",
                clauses
                    .iter()
                    .map(pubmed_expr)
                    .collect::<Planned<Vec<_>>>()?
                    .join(op)
            ))
        }
        SearchExpr::Not { clause } => Ok(format!("(all[sb] NOT {})", pubmed_expr(clause)?)),
        SearchExpr::Range {
            field: SearchField::Year,
            gte,
            lte,
        } => Ok(format!(
            "{}/01/01:{}/12/31[dp]",
            gte.as_deref().map(year).transpose()?.unwrap_or(1),
            lte.as_deref().map(year).transpose()?.unwrap_or(9999)
        )),
        SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field } => {
            let field = field.unwrap_or(SearchField::Any);
            if field == SearchField::Year {
                let y = year(value)?;
                return Ok(format!("{y}/01/01:{y}/12/31[dp]"));
            }
            let tag = match field {
                SearchField::Any => "Title/Abstract",
                SearchField::Title => "Title",
                SearchField::Creator => "Author",
                SearchField::Abstract => "Abstract",
                SearchField::Publication => "Journal",
                SearchField::Identifier if value.bytes().all(|b| b.is_ascii_digit()) => "PMID",
                SearchField::Identifier if value.to_ascii_lowercase().starts_with("10.") => "DOI",
                _ => {
                    return Err(unsupported(format!(
                        "PubMed does not support {field:?} as a structured search field"
                    )));
                }
            };
            Ok(format!("{}[{tag}]", literal(value)?))
        }
        _ => Err(unsupported("PubMed supports only publication year ranges")),
    }
}
fn pubmed(request: &DiscoveryRequest) -> Planned<BTreeMap<String, String>> {
    if request.filters.open_access.is_some() {
        return Err(unsupported(
            "PubMed free full text is not equivalent to open access",
        ));
    }
    let mut clauses = request
        .expression
        .as_ref()
        .map(pubmed_expr)
        .transpose()?
        .into_iter()
        .collect::<Vec<_>>();
    if request.filters.year_from.is_some() || request.filters.year_to.is_some() {
        clauses.push(format!(
            "{}/01/01:{}/12/31[dp]",
            request.filters.year_from.unwrap_or(1),
            request.filters.year_to.unwrap_or(9999)
        ));
    }
    if !request.filters.publication_types.is_empty() {
        clauses.push(format!(
            "({})",
            request
                .filters
                .publication_types
                .iter()
                .map(|v| {
                    let mapped = match v.trim().to_ascii_lowercase().as_str() {
                        "article" | "journal_article" => "Journal Article",
                        "clinical_trial" => "Clinical Trial",
                        _ => v,
                    };
                    literal(mapped).map(|v| format!("{v}[Publication Type]"))
                })
                .collect::<Planned<Vec<_>>>()?
                .join(" OR ")
        ));
    }
    Ok(BTreeMap::from([("term".into(), clauses.join(" AND "))]))
}

/// OpenAlex permits full Boolean text within one field, and AND between
/// fields. Cross-field OR/NOT cannot be flattened into comma-separated filters.
fn oa_expr(expr: &SearchExpr) -> Planned<BTreeMap<String, String>> {
    match expr {
        SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
            let parts = clauses.iter().map(oa_expr).collect::<Planned<Vec<_>>>()?;
            let is_or = matches!(expr, SearchExpr::Or { .. });
            if is_or
                && (parts.iter().any(|p| p.len() != 1)
                    || parts
                        .iter()
                        .any(|p| p.keys().next() != parts[0].keys().next())
                    || parts[0].contains_key("filter"))
            {
                return Err(unsupported(
                    "OpenAlex cannot express this cross-field OR; use a native query or select another source",
                ));
            }
            let mut combined: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for part in parts {
                for (key, value) in part {
                    combined.entry(key).or_default().push(value);
                }
            }
            Ok(combined
                .into_iter()
                .map(|(key, values)| {
                    let value = if key == "filter" {
                        values.join(",")
                    } else {
                        format!("({})", values.join(if is_or { " OR " } else { " AND " }))
                    };
                    (key, value)
                })
                .collect())
        }
        SearchExpr::Not { clause } => {
            let part = oa_expr(clause)?;
            if part.len() != 1 || part.contains_key("filter") {
                return Err(unsupported(
                    "OpenAlex cannot negate this structured field combination",
                ));
            }
            Ok(part
                .into_iter()
                .map(|(key, value)| (key, format!("NOT ({value})")))
                .collect())
        }
        SearchExpr::Range {
            field: SearchField::Year,
            gte,
            lte,
        } => Ok(BTreeMap::from([(
            "filter".into(),
            format!(
                "publication_year:{}-{}",
                gte.as_deref().map(year).transpose()?.unwrap_or(1),
                lte.as_deref().map(year).transpose()?.unwrap_or(9999)
            ),
        )])),
        SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field } => {
            let field = field.unwrap_or(SearchField::Any);
            let (key, value) = match field {
                SearchField::Any => ("search.exact", literal(value)?),
                SearchField::Title => ("search.title.exact", literal(value)?),
                SearchField::Abstract => (
                    "filter",
                    format!("abstract.search.exact:{}", literal(value)?),
                ),
                SearchField::Creator => (
                    "filter",
                    format!("raw_author_name.search:{}", literal(value)?),
                ),
                SearchField::Year => ("filter", format!("publication_year:{}", year(value)?)),
                SearchField::Identifier if value.starts_with("10.") => (
                    "filter",
                    format!("doi:{}", literal(&format!("https://doi.org/{value}"))?),
                ),
                _ => {
                    return Err(unsupported(format!(
                        "OpenAlex does not support {field:?} as a structured search field"
                    )));
                }
            };
            if key == "filter" && value.contains('|') {
                return Err(unsupported(
                    "filter value contains a source operator; use native mode",
                ));
            }
            Ok(BTreeMap::from([(key.into(), value)]))
        }
        _ => Err(unsupported(
            "OpenAlex supports only publication year ranges",
        )),
    }
}
fn openalex(request: &DiscoveryRequest) -> Planned<BTreeMap<String, String>> {
    let mut params = request
        .expression
        .as_ref()
        .map(oa_expr)
        .transpose()?
        .unwrap_or_default();
    let mut filters = params.remove("filter").into_iter().collect::<Vec<_>>();
    let f = &request.filters;
    if f.year_from.is_some() || f.year_to.is_some() {
        filters.push(format!(
            "publication_year:{}-{}",
            f.year_from.unwrap_or(1),
            f.year_to.unwrap_or(9999)
        ));
    }
    if let Some(oa) = f.open_access {
        filters.push(format!("open_access.is_oa:{oa}"));
    }
    if !f.publication_types.is_empty() {
        let types = f
            .publication_types
            .iter()
            .map(|v| match v.trim().to_ascii_lowercase().as_str() {
                "journal_article" | "journal article" => "article".into(),
                _ => v.trim().to_ascii_lowercase(),
            })
            .collect::<Vec<String>>();
        if types
            .iter()
            .any(|v| !v.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
        {
            return Err(unsupported(
                "OpenAlex does not support these publication types; use its native type identifiers",
            ));
        }
        filters.push(format!("type:{}", types.join("|")));
    }
    if !filters.is_empty() {
        params.insert("filter".into(), filters.join(","));
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(query: &str) -> DiscoveryRequest {
        DiscoveryRequest {
            query: query.into(),
            ..Default::default()
        }
    }

    #[test]
    fn pubmed_preserves_groups_fields_negation_and_years() {
        let p=plan("pubmed",&request("(title:asthma OR abstract:\"gene editing\") AND NOT author:Smith AND year:2020..2024")).unwrap();
        assert_eq!(
            p.parameters["term"],
            "((\"asthma\"[Title] OR \"gene editing\"[Abstract]) AND (all[sb] NOT \"Smith\"[Author]) AND 2020/01/01:2024/12/31[dp])"
        );
        assert_eq!(p.parameters["sort"], "relevance");
        assert!(plan("pubmed", &request("fulltext:asthma")).is_err());
        let mut oa = request("asthma");
        oa.filters.open_access = Some(true);
        assert_eq!(
            plan("pubmed", &oa).unwrap_err().kind,
            SourceFailureKind::Unsupported
        );
    }

    #[test]
    fn openalex_uses_boolean_text_and_explicit_exact_scope() {
        let p = plan(
            "openalex",
            &request(
                "(gene OR cancer) AND NOT mice AND title:\"clinical trial\" AND year:2020..2024",
            ),
        )
        .unwrap();
        assert_eq!(
            p.parameters["search.exact"],
            "((\"gene\" OR \"cancer\") AND NOT (\"mice\"))"
        );
        assert_eq!(p.parameters["search.title.exact"], "(\"clinical trial\")");
        assert_eq!(p.parameters["filter"], "publication_year:2020-2024");
        for query in [
            "title:gene OR author:Smith",
            "NOT (title:gene AND year:2020)",
            "note:gene",
        ] {
            assert_eq!(
                plan("openalex", &request(query)).unwrap_err().kind,
                SourceFailureKind::Unsupported,
                "{query}"
            );
        }
    }

    #[test]
    fn filters_and_native_parameters_are_not_silently_dropped() {
        let mut r = request("cancer");
        r.filters = DiscoveryFilters {
            year_from: Some(2020),
            year_to: Some(2024),
            publication_types: vec!["review".into(), "article".into()],
            open_access: Some(false),
        };
        let p = plan("openalex", &r).unwrap();
        assert_eq!(
            p.parameters["filter"],
            "publication_year:2020-2024,open_access.is_oa:false,type:review|article"
        );
        r.filters.open_access = None;
        r.filters.publication_types = vec!["Journal Article".into(), "Review".into()];
        assert_eq!(
            plan("openalex", &r).unwrap().parameters["filter"],
            "publication_year:2020-2024,type:article|review"
        );
        assert!(
            plan("pubmed", &r).unwrap().parameters["term"]
                .contains("\"Journal Article\"[Publication Type] OR \"Review\"[Publication Type]")
        );
        r.filters.publication_types = vec!["clinical_trial".into()];
        assert!(
            plan("pubmed", &r).unwrap().parameters["term"]
                .contains("\"Clinical Trial\"[Publication Type]")
        );
        assert_eq!(
            plan("openalex", &r).unwrap_err().kind,
            SourceFailureKind::Unsupported
        );
        let mut native = DiscoveryRequest {
            query: String::new(),
            sources: vec!["pubmed".into()],
            native: Some(NativeSearch {
                source: "pubmed".into(),
                parameters: BTreeMap::from([(
                    "term".into(),
                    "asthma[MeSH Terms] NOT review[pt]".into(),
                )]),
            }),
            ..Default::default()
        };
        assert_eq!(
            plan("pubmed", &native).unwrap().parameters["term"],
            "asthma[MeSH Terms] NOT review[pt]"
        );
        native
            .native
            .as_mut()
            .unwrap()
            .parameters
            .insert("api_key".into(), "not-allowed".into());
        assert_eq!(
            plan("pubmed", &native).unwrap_err().kind,
            SourceFailureKind::InvalidQuery
        );
        native.native.as_mut().unwrap().parameters.remove("api_key");
        native.filters.year_from = Some(2020);
        assert!(normalize(native).is_err());
        let empty = DiscoveryRequest {
            sources: vec!["pubmed".into()],
            native: Some(NativeSearch {
                source: "pubmed".into(),
                parameters: BTreeMap::from([("term".into(), " ".into())]),
            }),
            ..Default::default()
        };
        assert!(normalize(empty).is_err());
    }

    #[test]
    fn known_doi_input_is_an_identifier_instead_of_topic_text() {
        for query in [
            "https://doi.org/10.1000/Exact-paper",
            "doi:10.1000/Exact-paper",
            "HTTPS://DOI.ORG/10.1000/Exact-paper",
            "DOI:10.1000/Exact-paper",
            "10.1000/Exact-paper",
        ] {
            assert_eq!(
                plan("pubmed", &request(query)).unwrap().parameters["term"],
                "\"10.1000/Exact-paper\"[DOI]"
            );
            assert_eq!(
                plan("openalex", &request(query)).unwrap().parameters["filter"],
                "doi:\"https://doi.org/10.1000/Exact-paper\""
            );
        }
    }

    #[test]
    fn validation_bounds_sources_modes_and_cursor_fingerprints() {
        let a = plan("pubmed", &request("title:gene")).unwrap();
        let mut b = request("title:gene");
        b.page_size = 50;
        assert_ne!(a.fingerprint, plan("pubmed", &b).unwrap().fingerprint);
        let b = normalize(request("title:gene")).unwrap();
        assert_eq!(a, plan("pubmed", &b).unwrap());
        let mut r = request("gene");
        r.expression = Some(SearchExpr::Term {
            value: "cancer".into(),
            field: None,
        });
        assert!(normalize(r).is_err());
        for change in [0, 101] {
            let mut r = request("gene");
            r.page_size = change;
            assert!(normalize(r).is_err());
        }
        let mut r = request("gene");
        r.sources = vec!["pubmed".into(), "pubmed".into()];
        assert!(normalize(r).is_err());
        let mut r = request("gene");
        r.sources = vec!["biorxiv".into()];
        assert_eq!(
            plan("biorxiv", &r).unwrap_err().kind,
            SourceFailureKind::Unsupported
        );
        assert!(
            serde_json::from_str::<DiscoveryRequest>(r#"{"query":"gene","unsupported":true}"#)
                .is_err()
        );
    }
}
