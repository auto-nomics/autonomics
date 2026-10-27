use bib_base::{BibBase, Error};
use bib_types::{
    Article, Author, SaveSearchInput, SearchFilters, SearchFulltextStatus, SearchRequestV2,
};

fn author(last: &str) -> Author {
    Author {
        last_name: last.into(),
        fore_name: Some("Ada".into()),
        initials: None,
        affiliation: None,
        orcid: None,
        corresponding: false,
    }
}
async fn fixture() -> BibBase {
    let db = BibBase::open_in_memory().await.unwrap();
    for (id, tag, person, year, kind) in [
        ("a", "gene", "Lovelace", Some(2020), "Review"),
        ("b", "genetics", "Lovelace", Some(2024), "Journal Article"),
        ("c", "gene", "Hopper", None, "Review"),
    ] {
        let mut article = Article::new(id, format!("Study {id}"));
        article.keywords = vec![tag.into()];
        article.authors = vec![author(person)];
        article.year = year;
        article.pub_types = vec![kind.into(), kind.into()];
        db.upsert_article(&article).await.unwrap();
    }
    db
}
fn all() -> SearchRequestV2 {
    SearchRequestV2 {
        include_facets: true,
        include_articles: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn compatibility_and_collection_search_share_v2_boolean_field_and_phrase_semantics() {
    use bib_base::collections::CollectionQuery;
    use bib_types::{AddedBy, ArticleRole, Collection};
    let db = fixture().await;
    db.upsert_collection(&Collection::new("scope", "Scope"))
        .await
        .unwrap();
    for id in ["a", "b", "c"] {
        db.add_to_collection("scope", id, ArticleRole::Referenced, AddedBy::User, None)
            .await
            .unwrap();
    }
    for query in [
        "Study Study",
        "title:\"Study a\"",
        "author:Lovelace AND year:2020..2024",
        "(title:a OR title:b) AND NOT year:2020",
        "NOT author:Hopper",
        "tag:gene",
        "title:missing OR year:2024",
        "title:\"a Study\"",
    ] {
        let page = db
            .search_v2(SearchRequestV2 {
                query: query.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let expected = page
            .hits
            .iter()
            .map(|hit| hit.article_id.as_str())
            .collect::<Vec<_>>();
        let compat = db.search_articles(query, 100, 0).await.unwrap();
        assert_eq!(
            compat
                .iter()
                .map(|hit| hit.article_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "compat: {query}"
        );
        let collection = db
            .query_collection_articles(CollectionQuery {
                query: query.into(),
                collection_id: Some("scope".into()),
                limit: 100,
                offset: 0,
                include_descendants: false,
                unfiled: false,
            })
            .await
            .unwrap();
        assert_eq!(collection.total as usize, page.total, "count: {query}");
        assert_eq!(
            collection
                .articles
                .iter()
                .map(|article| article.id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "scope: {query}"
        );
        let tail = db.search_articles(query, 1, 1).await.unwrap();
        assert_eq!(
            tail.iter()
                .map(|hit| hit.article_id.as_str())
                .collect::<Vec<_>>(),
            expected.iter().skip(1).take(1).copied().collect::<Vec<_>>(),
            "paging: {query}"
        );
    }
    for query in ["title:(", "year:banana"] {
        assert!(db.search_articles(query, 20, 0).await.is_err());
        assert!(
            db.query_collection_articles(CollectionQuery {
                query: query.into(),
                collection_id: None,
                limit: 20,
                offset: 0,
                include_descendants: false,
                unfiled: false
            })
            .await
            .is_err()
        );
        // A failed transaction must release its snapshot for the next query.
        assert_eq!(db.search_articles("Study", 20, 0).await.unwrap().len(), 3);
    }
}

#[tokio::test]
async fn exact_filters_and_disjunctive_facets_precede_parent_pagination() {
    let db = fixture().await;
    let page = db.search_v2(all()).await.unwrap();
    assert_eq!(page.total, 3);
    let facets = page.facets.unwrap();
    let gene = facets
        .tags
        .iter()
        .find(|v| v.label == "gene")
        .unwrap()
        .value
        .parse()
        .unwrap();
    let genetics: i64 = facets
        .tags
        .iter()
        .find(|v| v.label == "genetics")
        .unwrap()
        .value
        .parse()
        .unwrap();
    let lovelace = facets
        .creators
        .iter()
        .find(|v| v.label == "Ada Lovelace")
        .unwrap()
        .value
        .parse()
        .unwrap();
    assert_eq!(
        facets
            .publication_types
            .iter()
            .find(|v| v.label == "Review")
            .unwrap()
            .count,
        2,
        "duplicate type labels must count the parent once"
    );
    assert_eq!(
        facets
            .years
            .iter()
            .find(|v| v.value == "unknown")
            .unwrap()
            .count,
        1
    );
    let mut request = all();
    request.page_size = 1;
    request.filters.tag_ids = vec![gene];
    let first = db.search_v2(request.clone()).await.unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(first.hits.len(), 1);
    assert_eq!(first.articles.len(), 1);
    assert_eq!(first.hits[0].article_id, first.articles[0].id);
    assert_eq!(
        first
            .facets
            .as_ref()
            .unwrap()
            .tags
            .iter()
            .find(|v| v.value == genetics.to_string())
            .unwrap()
            .count,
        1,
        "tag counts ignore tag selection"
    );
    request.cursor = first.next_cursor;
    let second = db.search_v2(request.clone()).await.unwrap();
    assert_ne!(first.hits[0].article_id, second.hits[0].article_id);
    assert!(second.next_cursor.is_none());
    request.cursor = None;
    request.filters.creator_ids = vec![lovelace];
    let intersect = db.search_v2(request.clone()).await.unwrap();
    assert_eq!(intersect.total, 1);
    assert_eq!(intersect.hits[0].article_id, "a");
    assert_eq!(
        intersect
            .facets
            .unwrap()
            .tags
            .iter()
            .find(|v| v.value == genetics.to_string())
            .unwrap()
            .count,
        1,
        "other filters remain active for disjunctive counts"
    );
    request.filters.tag_ids.push(genetics);
    assert_eq!(
        db.search_v2(request.clone()).await.unwrap().total,
        2,
        "selections within a facet are OR"
    );
    request.filters.year_from = Some(2021);
    let page = db.search_v2(request.clone()).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].article_id, "b");
    request.filters = SearchFilters {
        year_to: Some(2021),
        ..Default::default()
    };
    assert_eq!(
        db.search_v2(request.clone()).await.unwrap().total,
        1,
        "unknown years are excluded by bounds"
    );
    request.filters.year_from = Some(2025);
    assert!(matches!(
        db.search_v2(request).await,
        Err(Error::InvalidSearchQuery(_))
    ));
}

#[tokio::test]
async fn fulltext_annotation_library_and_type_filters_have_explicit_semantics() {
    let db = fixture().await;
    db.add_annotation(
        "a",
        bib_types::AnnotationKind::Note,
        "evidence",
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    db.upsert_fulltext(&bib_types::FullText {
        article_id: "b".into(),
        attachment_key: None,
        file_path: "/articles/b.pdf".into(),
        file_format: bib_types::FileFormat::Pdf,
        text_content: Some("searchable methods".into()),
        source: bib_types::FullTextSource::UserUpload,
        file_hash: None,
        file_size: None,
        uploaded_at: None,
        extract_status: None,
        text_format: None,
        extracted_by: None,
        extract_error: None,
    })
    .await
    .unwrap();
    let mut req = all();
    req.filters.has_annotations = Some(true);
    assert_eq!(
        db.search_v2(req.clone()).await.unwrap().hits[0].article_id,
        "a"
    );
    req.filters = SearchFilters {
        has_fulltext: Some(true),
        ..Default::default()
    };
    let page = db.search_v2(req.clone()).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].article_id, "b");
    req.filters = SearchFilters {
        fulltext_status: Some(SearchFulltextStatus::Indexed),
        ..Default::default()
    };
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 1);
    req.filters = SearchFilters {
        fulltext_status: Some(SearchFulltextStatus::Missing),
        ..Default::default()
    };
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 2);
    req.filters = SearchFilters {
        publication_types: vec!["Review".into()],
        ..Default::default()
    };
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 2);
    req.scope.library_id = Some(99);
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 0);
    req.scope.library_id = Some(1);
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 2);
    req.scope.library_id = Some(0);
    assert!(db.search_v2(req).await.is_err());
}

#[tokio::test]
async fn saved_rules_are_dynamic_durable_and_use_optimistic_concurrency() {
    let dir = std::env::temp_dir().join(format!("bib-saved-rules-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("library.db");
    let db = BibBase::open(path.to_str().unwrap()).await.unwrap();
    let mut req = all();
    req.query = "title:Study".into();
    req.cursor = Some("transient-cursor-must-not-be-saved".into());
    let rule = db
        .save_search(
            None,
            SaveSearchInput {
                name: "  My living review  ".into(),
                request: req,
                expected_version: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(rule.name, "My living review");
    assert!(rule.request.cursor.is_none());
    assert_eq!(rule.schema_version, 1);
    assert_eq!(db.search_v2(rule.request.clone()).await.unwrap().total, 0);
    db.upsert_article(&Article::new("a", "Study of methods"))
        .await
        .unwrap();
    assert_eq!(db.search_v2(rule.request.clone()).await.unwrap().total, 1);
    let other = BibBase::open(path.to_str().unwrap()).await.unwrap();
    let persisted = other.list_saved_searches().await.unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].id, rule.id);
    let input = SaveSearchInput {
        name: "Updated name".into(),
        request: rule.request.clone(),
        expected_version: Some(1),
    };
    let updated = db.save_search(Some(&rule.id), input.clone()).await.unwrap();
    assert_eq!(updated.version, 2);
    assert!(matches!(
        other.save_search(Some(&rule.id), input).await,
        Err(Error::SearchConflict(_))
    ));
    assert!(matches!(
        db.delete_saved_search(&rule.id, 1).await,
        Err(Error::SearchConflict(_))
    ));
    other.delete_saved_search(&rule.id, 2).await.unwrap();
    assert!(db.list_saved_searches().await.unwrap().is_empty());
    assert!(
        db.get_article("a").await.unwrap().is_some(),
        "deleting a rule must never remove articles"
    );
    assert!(matches!(
        db.delete_saved_search(&rule.id, 2).await,
        Err(Error::NotFound(_))
    ));
    drop(other);
    drop(db);
    // Let the actor close its handles before cleaning this isolated fixture.
    tokio::task::yield_now().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn invalid_saved_rules_never_persist() {
    let db = fixture().await;
    for (name, query) in [
        ("", "Study"),
        ("valid", "unknownfield:x"),
        ("valid", "title:("),
    ] {
        let input = SaveSearchInput {
            name: name.into(),
            request: SearchRequestV2 {
                query: query.into(),
                ..Default::default()
            },
            expected_version: None,
        };
        assert!(matches!(
            db.save_search(None, input).await,
            Err(Error::InvalidSearchQuery(_))
        ));
    }
    assert!(db.list_saved_searches().await.unwrap().is_empty());
}

async fn attachment(db: &BibBase, name: &str, text: &str, preferred: bool) -> String {
    let file = db
        .create_zotero_attachment(
            bib_base::NewAttachment {
                article_id: "a".into(),
                title: Some(name.into()),
                filename: Some(name.into()),
                content_type: Some("application/pdf".into()),
                file_format: bib_types::FileFormat::Pdf,
                link_mode: bib_base::LINK_MODE_IMPORTED_FILE,
                path: Some(format!("/articles/{name}")),
                source_url: None,
                vfs_path: None,
                storage_hash: None,
                file_size: None,
            },
            preferred,
        )
        .await
        .unwrap();
    db.record_attachment_extraction_success(
        &file.key,
        text,
        bib_types::TextFormat::Plain,
        "fixture",
        "/extracted.txt",
    )
    .await
    .unwrap();
    file.key
}

#[tokio::test]
async fn evidence_keeps_each_attachment_and_annotation_with_safe_unicode_ranges() {
    let db = fixture().await;
    let first = attachment(&db, "first.pdf", "😀 前缀 肿瘤免疫治疗 distinctive", true).await;
    let second = attachment(&db, "second.pdf", "肿瘤免疫治疗 distinctive", false).await;
    let note = db
        .add_annotation(
            "a",
            bib_types::AnnotationKind::Highlight,
            "肿瘤免疫治疗 distinctive",
            Some(7),
            None,
            None,
            Some(&second),
        )
        .await
        .unwrap();
    let mut search = all();
    search.query = "distinctive".into();
    search.page_size = 1;
    let page = db.search_v2(search.clone()).await.unwrap();
    let generation = page.index_generation;
    let mut request = bib_types::SearchEvidenceRequest {
        search,
        index_generation: Some(generation.clone()),
    };
    let mut evidence = Vec::new();
    loop {
        let page = db.search_evidence("a", request.clone()).await.unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.index_generation, generation);
        evidence.extend(page.evidence);
        if page.next_cursor.is_none() {
            break;
        }
        request.search.cursor = page.next_cursor;
    }
    assert!(
        evidence
            .iter()
            .any(|e| e.attachment_key.as_deref() == Some(&first) && e.annotation_key.is_none())
    );
    assert!(
        evidence
            .iter()
            .any(|e| e.attachment_key.as_deref() == Some(&second) && e.annotation_key.is_none())
    );
    let anchored = evidence
        .iter()
        .find(|e| e.annotation_id.as_deref() == Some(&note.id))
        .unwrap();
    assert_eq!(anchored.page, Some(7));
    assert_eq!(anchored.attachment_key.as_deref(), Some(second.as_str()));
    for entry in &evidence {
        assert_eq!(entry.content_revision, generation);
        assert!(!entry.highlight_ranges.is_empty());
        let chars = entry.snippet.chars().collect::<Vec<_>>();
        for range in &entry.highlight_ranges {
            assert_eq!(
                chars[range.start..range.end].iter().collect::<String>(),
                "distinctive"
            );
        }
        if entry.annotation_id.is_none() {
            assert_eq!(entry.page, None);
            assert_eq!(entry.location_status, "text_search");
        }
    }
    request.search.cursor = None;
    request.search.query = "fulltext:\"肿瘤免疫治疗\"".into();
    request.search.page_size = 20;
    let unicode = db.search_evidence("a", request.clone()).await.unwrap();
    assert_eq!(unicode.total, 2);
    for e in &unicode.evidence {
        let chars = e.snippet.chars().collect::<Vec<_>>();
        assert!(
            e.highlight_ranges
                .iter()
                .any(|r| chars[r.start..r.end].iter().collect::<String>() == "肿瘤免疫治疗")
        );
    }
    request.search.filters.has_annotations = Some(false);
    assert!(matches!(
        db.search_evidence("a", request.clone()).await,
        Err(Error::NotFound(_))
    ));
    request.search.filters = Default::default();
    db.upsert_article(&Article::new("a", "Changed study"))
        .await
        .unwrap();
    assert!(
        matches!(
            db.search_evidence("a", request).await,
            Err(Error::InvalidSearchQuery(_))
        ),
        "old evidence must expire after metadata/content edits"
    );
}

#[tokio::test]
async fn evidence_respects_successful_boolean_branches_and_same_child_rules() {
    let db = fixture().await;
    attachment(&db, "first.pdf", "alpha uniqueleft", true).await;
    attachment(&db, "second.pdf", "beta uniqueright", false).await;
    let search = SearchRequestV2 {
        query: "(fulltext:alpha AND fulltext:beta) OR fulltext:gamma".into(),
        ..Default::default()
    };
    let mut req = bib_types::SearchEvidenceRequest {
        search,
        index_generation: None,
    };
    let page = db.search_evidence("a", req.clone()).await.unwrap();
    assert_eq!(page.total, 2);
    req.search.scope.require_same_child = true;
    assert!(matches!(
        db.search_evidence("a", req.clone()).await,
        Err(Error::NotFound(_))
    ));
    req.search.scope.require_same_child = false;
    req.search.query = "(fulltext:alpha AND fulltext:absent) OR fulltext:beta".into();
    let page = db.search_evidence("a", req.clone()).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.evidence[0].matched_query, "beta");
    req.search.query = "fulltext:alpha NOT fulltext:beta".into();
    assert!(matches!(
        db.search_evidence("a", req).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn cached_facets_replay_updates_deletes_and_extraction_transitions() {
    let db = fixture().await;
    let initial = db.search_v2(all()).await.unwrap();
    let facets = initial.facets.unwrap();
    let gene = facets
        .tags
        .iter()
        .find(|v| v.label == "gene")
        .unwrap()
        .value
        .parse::<i64>()
        .unwrap();
    let mut req = all();
    req.filters.tag_ids = vec![gene];
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 2);
    let mut updated = Article::new("a", "Study changed");
    updated.keywords = vec!["changed".into()];
    db.upsert_article(&updated).await.unwrap();
    let after = db.search_v2(req.clone()).await.unwrap();
    assert_eq!(after.total, 1);
    assert_eq!(after.hits[0].article_id, "c");
    assert!(
        after
            .facets
            .unwrap()
            .tags
            .iter()
            .any(|v| v.label == "changed")
    );
    db.delete_article("c").await.unwrap();
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 0);
    let key = attachment(&db, "state.pdf", "ready material", true).await;
    req.filters = SearchFilters {
        fulltext_status: Some(SearchFulltextStatus::Indexed),
        ..Default::default()
    };
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 1);
    db.restart_attachment_extraction(&key).await.unwrap();
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 0);
    req.filters.fulltext_status = Some(SearchFulltextStatus::Pending);
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 1);
    db.record_attachment_extraction_failure(&key, "fixture failure")
        .await
        .unwrap();
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 0);
    req.filters.fulltext_status = Some(SearchFulltextStatus::Failed);
    assert_eq!(db.search_v2(req).await.unwrap().total, 1);
}

#[tokio::test]
async fn evidence_handles_negation_polarity_same_child_branches_and_year_metadata() {
    let db = fixture().await;
    attachment(&db, "first.pdf", "alpha uniqueleft", true).await;
    attachment(&db, "second.pdf", "beta uniqueright", false).await;
    for (query, same_child, expected) in [
        (
            "(fulltext:alpha AND fulltext:beta) OR fulltext:uniqueleft",
            true,
            "uniqueleft",
        ),
        (
            "NOT (NOT fulltext:alpha OR fulltext:absent)",
            false,
            "alpha",
        ),
        ("NOT NOT fulltext:beta", true, "beta"),
        ("year:2020", false, "2020"),
        ("year:2019..2021", false, "2020"),
    ] {
        let mut search = SearchRequestV2 {
            query: query.into(),
            ..Default::default()
        };
        search.scope.require_same_child = same_child;
        let page = db
            .search_evidence(
                "a",
                bib_types::SearchEvidenceRequest {
                    search,
                    index_generation: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 1, "{query}");
        assert_eq!(page.evidence[0].matched_query, expected, "{query}");
        if expected == "2020" {
            assert_eq!(page.evidence[0].matched_field, "year_text");
            assert_eq!(page.evidence[0].snippet, "2020");
        }
    }
}

#[tokio::test]
async fn mixed_attachment_states_preserve_failure_visibility_alongside_indexed_text() {
    let db = fixture().await;
    attachment(&db, "ready.pdf", "ready content", true).await;
    let failed = attachment(&db, "failed.pdf", "old content", false).await;
    db.restart_attachment_extraction(&failed).await.unwrap();
    db.record_attachment_extraction_failure(&failed, "failed fixture")
        .await
        .unwrap();
    let mut req = all();
    req.filters.fulltext_status = Some(SearchFulltextStatus::Indexed);
    let page = db.search_v2(req.clone()).await.unwrap();
    assert_eq!(page.total, 1);
    let facets = page.facets.unwrap();
    assert_eq!(
        facets
            .fulltext_status
            .iter()
            .find(|v| v.value == "failed")
            .unwrap()
            .count,
        1
    );
    assert_eq!(
        facets
            .fulltext_status
            .iter()
            .find(|v| v.value == "indexed")
            .unwrap()
            .count,
        1
    );
    req.filters.fulltext_status = Some(SearchFulltextStatus::Failed);
    assert_eq!(
        db.search_v2(req).await.unwrap().total,
        1,
        "an indexed sibling must not hide a failed attachment"
    );
}

#[tokio::test]
async fn filtered_export_uses_full_snapshot_scope_and_requested_order() {
    let db = fixture().await;
    let page = db.search_v2(all()).await.unwrap();
    let facets = page.facets.unwrap();
    let gene = facets
        .tags
        .iter()
        .find(|v| v.label == "gene")
        .unwrap()
        .value
        .parse()
        .unwrap();
    let mut req = SearchRequestV2 {
        query: "title:Study".into(),
        page_size: 1,
        sort: bib_types::SearchSort::Newest,
        filters: SearchFilters {
            tag_ids: vec![gene],
            ..Default::default()
        },
        ..Default::default()
    };
    let first = db.search_v2(req.clone()).await.unwrap();
    assert_eq!(first.hits.len(), 1);
    let exported = db.export_search(req.clone()).await.unwrap();
    assert_eq!(
        exported.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "c"],
        "export must include more than the current page and preserve filtering/order"
    );
    req.cursor = first.next_cursor;
    assert!(matches!(
        db.export_search(req).await,
        Err(Error::InvalidSearchQuery(_))
    ));
    let mut bulk = Vec::new();
    for i in 0..205 {
        bulk.push(Article::new(format!("bulk-{i:03}"), "Bulktarget"));
    }
    db.upsert_articles(&bulk).await.unwrap();
    let exported = db
        .export_search(SearchRequestV2 {
            query: "title:Bulktarget".into(),
            page_size: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(exported.len(), 205, "no implicit 200/500 result cap");
}

#[tokio::test]
async fn quick_field_modes_limit_unfielded_terms_without_changing_explicit_fields() {
    use bib_types::SearchField;
    let db = fixture().await;
    attachment(&db, "body.pdf", "uniquebody", true).await;
    let mut req = SearchRequestV2 {
        query: "uniquebody".into(),
        ..Default::default()
    };
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 1);
    req.default_fields = vec![
        SearchField::Title,
        SearchField::Creator,
        SearchField::Publication,
        SearchField::Year,
    ];
    assert_eq!(db.search_v2(req.clone()).await.unwrap().total, 0);
    req.query = "fulltext:uniquebody".into();
    assert_eq!(
        db.search_v2(req.clone()).await.unwrap().total,
        1,
        "explicit fields override the default field mode"
    );
    req.query = "Lovelace".into();
    assert_eq!(
        db.search_v2(req.clone()).await.unwrap().total,
        2,
        "non-numeric words skip the numeric year alternative"
    );
    req.query = "2020".into();
    assert_eq!(
        db.search_v2(req.clone()).await.unwrap().hits[0].article_id,
        "a"
    );
    req.query = "NOT Lovelace".into();
    let page = db.search_v2(req.clone()).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].article_id, "c");
    req.query = "\"Study a\"".into();
    assert_eq!(
        db.search_v2(req.clone()).await.unwrap().total,
        1,
        "default fields preserve phrase adjacency"
    );
    let rule = db
        .save_search(
            None,
            SaveSearchInput {
                name: "Narrow fields".into(),
                request: req.clone(),
                expected_version: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(db.search_v2(rule.request).await.unwrap().total, 1);
    req.default_fields = vec![SearchField::Any];
    assert!(db.search_v2(req).await.is_err());
}
