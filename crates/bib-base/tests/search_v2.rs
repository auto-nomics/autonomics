use bib_base::BibBase;
use bib_types::{
    AnnotationKind, Article, Author, Identifier, SearchExpr, SearchField, SearchRequestV2,
    SearchSort,
};

fn term(value: &str, field: SearchField) -> SearchExpr {
    SearchExpr::Term {
        value: value.into(),
        field: Some(field),
    }
}
fn request(expr: SearchExpr) -> SearchRequestV2 {
    SearchRequestV2 {
        expression: Some(expr),
        ..Default::default()
    }
}
async fn fixture() -> BibBase {
    let db = BibBase::open_in_memory().await.unwrap();
    let mut a = Article::new("a", "Precise gene editing");
    a.abstract_text = Some("Neutral context about synthesis".into());
    a.journal = Some("Smith Methods".into());
    a.year = Some(2020);
    a.authors = vec![Author {
        last_name: "Curie".into(),
        fore_name: Some("Marie".into()),
        initials: None,
        affiliation: None,
        orcid: None,
        corresponding: false,
    }];
    a.keywords = vec!["CRISPR".into()];
    a.identifiers = vec![Identifier::doi("10.1000/exact-a")];
    let mut b = Article::new("b", "Gene regulation and editing");
    b.abstract_text = Some("Curie with CRISPR mentioned in the abstract".into());
    b.journal = Some("Other Methods".into());
    b.year = Some(2024);
    let mut c = Article::new("c", "Editing tools");
    c.year = Some(2022);
    db.upsert_articles(&[a, b, c]).await.unwrap();
    db
}
fn ids(page: &bib_types::SearchPageV2) -> Vec<&str> {
    page.hits
        .iter()
        .map(|hit| hit.article_id.as_str())
        .collect()
}

#[tokio::test]
async fn fields_phrases_identifiers_and_years_preserve_query_intent() {
    let db = fixture().await;
    for (value, field, expected) in [
        ("Curie", SearchField::Creator, vec!["a"]),
        ("Smith", SearchField::Creator, vec![]),
        ("Smith", SearchField::Publication, vec!["a"]),
        ("CRISPR", SearchField::Keyword, vec!["a"]),
        (
            "https://doi.org/10.1000/exact-a",
            SearchField::Identifier,
            vec!["a"],
        ),
        ("10.1000/exact", SearchField::Identifier, vec![]),
        ("2024", SearchField::Year, vec!["b"]),
    ] {
        let page = db.search_v2(request(term(value, field))).await.unwrap();
        assert_eq!(ids(&page), expected, "{field:?}: {value}");
    }
    let phrase = SearchExpr::Phrase {
        value: "gene editing".into(),
        field: Some(SearchField::Title),
    };
    assert_eq!(
        ids(&db.search_v2(request(phrase)).await.unwrap()),
        vec!["a"]
    );
    let range = SearchExpr::Range {
        field: SearchField::Year,
        gte: Some("2021".into()),
        lte: Some("2024".into()),
    };
    let mut q = request(range);
    q.sort = SearchSort::Newest;
    assert_eq!(ids(&db.search_v2(q).await.unwrap()), vec!["b", "c"]);
}

#[tokio::test]
async fn boolean_exclusions_apply_to_whole_articles_and_notes_keep_distinct_keys() {
    let db = fixture().await;
    let first = db
        .add_annotation(
            "a",
            AnnotationKind::Note,
            "quartzmarker",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let second = db
        .add_annotation(
            "a",
            AnnotationKind::Note,
            "rubymarker",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let cross_child = SearchExpr::And {
        clauses: vec![
            term("quartzmarker", SearchField::Note),
            term("rubymarker", SearchField::Note),
        ],
    };
    let mut q = request(cross_child);
    assert_eq!(ids(&db.search_v2(q.clone()).await.unwrap()), vec!["a"]);
    q.scope.require_same_child = true;
    assert!(db.search_v2(q).await.unwrap().hits.is_empty());
    for (text, annotation) in [("quartzmarker", first), ("rubymarker", second)] {
        let page = db
            .search_v2(request(term(text, SearchField::Note)))
            .await
            .unwrap();
        assert_eq!(page.hits[0].anchor_key, annotation.item_key);
    }
    let excluded = SearchExpr::And {
        clauses: vec![
            term("editing", SearchField::Title),
            SearchExpr::Not {
                clause: Box::new(term("rubymarker", SearchField::Note)),
            },
        ],
    };
    let mut q = request(excluded);
    q.scope.require_same_child = true;
    q.sort = SearchSort::Title;
    assert_eq!(ids(&db.search_v2(q).await.unwrap()), vec!["c", "b"]);
    let either = SearchExpr::Or {
        clauses: vec![
            term("Curie", SearchField::Creator),
            term("tools", SearchField::Title),
        ],
    };
    let mut q = request(either);
    q.sort = SearchSort::Title;
    assert_eq!(ids(&db.search_v2(q).await.unwrap()), vec!["c", "a"]);
}

#[tokio::test]
async fn cursor_pages_are_disjoint_and_reject_changed_queries_and_index_updates() {
    let db = fixture().await;
    let mut q = request(term("editing", SearchField::Title));
    q.page_size = 1;
    let first = db.search_v2(q.clone()).await.unwrap();
    assert_eq!(first.total, 3);
    q.cursor = first.next_cursor.clone();
    let second = db.search_v2(q.clone()).await.unwrap();
    assert_ne!(ids(&first), ids(&second));
    let mut other = q.clone();
    other.sort = SearchSort::Title;
    assert!(db.search_v2(other).await.is_err());
    db.upsert_article(&Article::new("d", "Editing update"))
        .await
        .unwrap();
    assert!(db.search_v2(q).await.is_err());
}

#[tokio::test]
async fn exclusions_and_empty_queries_work_without_positive_terms() {
    let db = fixture().await;
    let mut q = request(SearchExpr::Not {
        clause: Box::new(term("gene", SearchField::Title)),
    });
    q.sort = SearchSort::Title;
    assert_eq!(ids(&db.search_v2(q).await.unwrap()), vec!["c"]);
    let page = db.search_v2(SearchRequestV2::default()).await.unwrap();
    assert_eq!(page.total, 3);
    let mut unsupported = SearchRequestV2::default();
    unsupported.sort = SearchSort::CitationCount;
    assert!(db.search_v2(unsupported).await.is_err());
}

#[tokio::test]
async fn text_queries_parse_precedence_fields_ranges_and_report_syntax_errors() {
    let db = fixture().await;
    for (query, expected) in [
        (
            "author:Curie OR title:tools AND year:2022..2024",
            vec!["c", "a"],
        ),
        ("title:(gene OR tools) NOT year:2020", vec!["c", "b"]),
        ("title:\"gene editing\"", vec!["a"]),
        ("doi:10.1000/exact-a", vec!["a"]),
        ("year:>=2023", vec!["b"]),
    ] {
        let page = db
            .search_v2(SearchRequestV2 {
                query: query.into(),
                sort: SearchSort::Title,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(ids(&page), expected, "{query}");
    }
    for query in [
        "title:(gene OR)",
        "title:\"gene",
        "year:2025..2020",
        "author:",
        "NOT",
        "title:x OR",
        "typo:x",
    ] {
        assert!(
            db.search_v2(SearchRequestV2 {
                query: query.into(),
                ..Default::default()
            })
            .await
            .is_err(),
            "must reject {query}"
        );
    }
}

#[tokio::test]
async fn phrases_keep_repeated_words_and_exclusions_do_not_remove_nonadjacent_matches() {
    let db = BibBase::open_in_memory().await.unwrap();
    db.upsert_articles(&[
        Article::new("a", "Had had evidence"),
        Article::new("b", "Had evidence"),
        Article::new("c", "Had further had evidence"),
    ])
    .await
    .unwrap();
    for (query, expected) in [
        ("title:\"had had\"", vec!["a"]),
        ("NOT title:\"had had\"", vec!["b", "c"]),
    ] {
        let page = db
            .search_v2(SearchRequestV2 {
                query: query.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(ids(&page), expected);
    }
}

#[tokio::test]
async fn collection_scope_precedes_pagination_and_membership_changes_expire_cursors() {
    use bib_types::{AddedBy, ArticleRole, Collection};
    let db = fixture().await;
    let root = Collection::new("root", "Review");
    let mut child = Collection::new("child", "Subtopic");
    child.parent_id = Some("root".into());
    db.upsert_collection(&root).await.unwrap();
    db.upsert_collection(&child).await.unwrap();
    db.add_to_collection("root", "a", ArticleRole::Referenced, AddedBy::User, None)
        .await
        .unwrap();
    db.add_to_collection("child", "b", ArticleRole::Referenced, AddedBy::User, None)
        .await
        .unwrap();
    let mut q = request(term("editing", SearchField::Title));
    q.scope.collection_id = Some("root".into());
    q.page_size = 1;
    assert_eq!(db.search_v2(q.clone()).await.unwrap().total, 1);
    q.scope.include_descendants = true;
    let first = db.search_v2(q.clone()).await.unwrap();
    assert_eq!(first.total, 2);
    q.cursor = first.next_cursor;
    db.add_to_collection("root", "c", ArticleRole::Referenced, AddedBy::User, None)
        .await
        .unwrap();
    assert!(db.search_v2(q).await.is_err());
    let mut unfiled = SearchRequestV2::default();
    unfiled.scope.unfiled = true;
    assert_eq!(db.search_v2(unfiled).await.unwrap().total, 0);
}

#[tokio::test]
async fn snippets_use_word_and_phrase_offsets_including_unicode_case_folding() {
    let db = BibBase::open_in_memory().await.unwrap();
    let mut article = Article::new("a", "Unrelated title");
    article.abstract_text = Some(format!(
        "{} GENE evidence at the actual match. {} Βήτα evidence. {} Gene editing at the phrase target.",
        "Genomes in background. ".repeat(30),
        "Background. ".repeat(30),
        "Gene elsewhere. ".repeat(30)
    ));
    db.upsert_article(&article).await.unwrap();
    for (query, expected) in [
        ("abstract:gene", "GENE evidence"),
        ("abstract:βήτα", "Βήτα evidence"),
        (
            "abstract:\"gene editing\"",
            "Gene editing at the phrase target",
        ),
    ] {
        let page = db
            .search_v2(SearchRequestV2 {
                query: query.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert!(
            page.hits[0].snippet.contains(expected),
            "{query}: {}",
            page.hits[0].snippet
        );
    }
}

#[tokio::test]
async fn explicit_legacy_rollout_mode_keeps_basic_search_and_disables_v2_and_rebuild() {
    let db = BibBase::open_with_search_engine(":memory:", bib_base::SearchEngineMode::Legacy)
        .await
        .unwrap();
    db.upsert_articles(&[
        Article::new("a", "Rollbackmarker evidence"),
        Article::new("b", "Other evidence"),
    ])
    .await
    .unwrap();
    let hits = db
        .search_articles("rollbackmarker rollbackmarker", 10, 0)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].article_id, "a");
    assert_eq!(hits[0].score, 0.0);
    assert_eq!(db.search_articles("", 10, 0).await.unwrap().len(), 2);
    assert!(db.search_v2(SearchRequestV2::default()).await.is_err());
    assert!(db.rebuild_search_index().await.is_err());
    let status = db.search_index_status().await.unwrap();
    assert_eq!(status.engine, "legacy");
    assert_eq!(status.indexed_documents, 0);
}

#[tokio::test]
async fn chinese_words_phrases_and_mixed_script_offsets_share_the_same_analyzer() {
    let db = BibBase::open_in_memory().await.unwrap();
    let mut a = Article::new("a", "APOE相关肿瘤免疫治疗研究");
    a.abstract_text = Some(format!(
        "{}肿瘤免疫治疗显示疗效。",
        "背景资料。".repeat(100)
    ));
    let b = Article::new("b", "肿瘤细胞与免疫反应");
    db.upsert_articles(&[a, b]).await.unwrap();
    for (query, expected) in [
        ("title:免疫", vec!["a", "b"]),
        ("title:\"肿瘤免疫\"", vec!["a"]),
        ("title:APOE AND title:治疗", vec!["a"]),
        ("abstract:\"肿瘤免疫治疗\"", vec!["a"]),
    ] {
        let page = db
            .search_v2(SearchRequestV2 {
                query: query.into(),
                sort: SearchSort::Title,
                ..Default::default()
            })
            .await
            .unwrap();
        let mut actual = ids(&page);
        actual.sort();
        assert_eq!(actual, expected, "{query}");
        if query.starts_with("abstract:") {
            assert!(page.hits[0].snippet.contains("肿瘤免疫治疗"));
        }
    }
    assert_eq!(
        db.search_articles("肿瘤免疫", 10, 0).await.unwrap().len(),
        2
    );
}

#[tokio::test]
async fn scientific_symbols_require_adjacent_parts_and_keep_exact_gene_word_boundaries() {
    let db = BibBase::open_in_memory().await.unwrap();
    db.upsert_articles(&[
        Article::new("a", "IL-6 HLA-A*02:01 α-synuclein p53"),
        Article::new(
            "b",
            "IL-10 chromosome 6 HLA-B*02:01 α kinase synuclein p530",
        ),
    ])
    .await
    .unwrap();
    for query in ["IL-6", "HLA-A*02:01", "α-synuclein", "p53"] {
        let page = db
            .search_v2(SearchRequestV2 {
                query: query.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(ids(&page), vec!["a"], "{query}");
        assert_eq!(
            db.search_articles(query, 10, 0)
                .await
                .unwrap()
                .iter()
                .map(|hit| hit.article_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"],
            "legacy adapter: {query}"
        );
    }
}

#[tokio::test]
async fn metadata_phrases_do_not_cross_authors_keywords_or_tags() {
    let db = BibBase::open_in_memory().await.unwrap();
    let author = |last: &str, first: &str| Author {
        last_name: last.into(),
        fore_name: Some(first.into()),
        initials: None,
        affiliation: None,
        orcid: None,
        corresponding: false,
    };
    let mut a = Article::new("a", "Unrelated first record");
    a.authors = vec![author("Alpha", "Beta"), author("Gamma", "Delta")];
    a.keywords = vec!["gene".into(), "editing".into()];
    let mut b = Article::new("b", "Unrelated second record");
    b.authors = vec![author("Beta", "Gamma")];
    b.keywords = vec![
        "gene".into(),
        format!("editing {}", "padding ".repeat(100)),
        "gene editing target".into(),
    ];
    db.upsert_articles(&[a, b]).await.unwrap();
    for query in [
        "author:\"Beta Gamma\"",
        "keyword:\"gene editing\"",
        "tag:\"gene editing\"",
    ] {
        let page = db
            .search_v2(SearchRequestV2 {
                query: query.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(ids(&page), vec!["b"], "{query}");
        assert!(!page.hits[0].snippet.contains('\u{1e}'));
        if !query.starts_with("author:") {
            assert!(
                page.hits[0].snippet.contains("gene editing target"),
                "{}",
                page.hits[0].snippet
            );
        }
    }
}
