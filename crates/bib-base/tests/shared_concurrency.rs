//! Integration tests for `BibShared` — verifies that one shared bundle
//! backs many concurrent agent-like tasks without losing writes or
//! deadlocking.
//!
//! These tests exercise the regression behind the recent refactor:
//! previously every spawned agent reopened `BibBase::open` + rebuilt
//! `LiteratureGateway::with_default_sources`, so N agents meant N
//! independent libSQL connections contending on the WAL. Now all agents
//! share the same `Arc<BibShared>`.

use std::sync::Arc;

use bib_base::BibShared;
use bib_types::{
    Article, ArticleSource, Author, Identifier,
};

fn sample_article(id: &str) -> Article {
    let mut art = Article::new(id, &format!("Paper {id}"));
    art.authors.push(Author {
        last_name: "Smith".into(),
        fore_name: Some("John A".into()),
        initials: Some("JA".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    art.identifiers
        .push(Identifier::doi(format!("10.1000/{id}")));
    art.identifiers.push(Identifier::pmid(id.to_string()));
    art.abstract_text = Some(format!("Abstract for {id}."));
    art.year = Some(2024);
    art.journal = Some("Nature Genetics".into());
    art.source = ArticleSource::Pubmed;
    art
}

/// Two `open_in_memory()` calls produce **distinct** bundles — sharing
/// only happens when callers reuse the same `Arc<BibShared>`. This pins
/// down the contract so future maintainers don't confuse
/// "shared across agents in one host" with "shared across the world".
#[tokio::test]
async fn open_in_memory_yields_independent_bundles() {
    let a = BibShared::open_in_memory().await.unwrap();
    let b = BibShared::open_in_memory().await.unwrap();
    assert!(!Arc::ptr_eq(&a.bib, &b.bib));
    assert!(!Arc::ptr_eq(&a.gateway, &b.gateway));
}

/// `BibShared` is `Clone` and shares the same inner `Arc`s across all
/// clones — exactly what a multi-agent host needs when handing the
/// shared handle to each spawned agent.
#[tokio::test]
async fn clones_share_underlying_arcs() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let c1 = shared.clone();
    let c2 = shared.clone();
    assert!(Arc::ptr_eq(&shared.bib, &c1.bib));
    assert!(Arc::ptr_eq(&shared.bib, &c2.bib));
    assert!(Arc::ptr_eq(&shared.gateway, &c1.gateway));
    assert!(Arc::ptr_eq(&shared.gateway, &c2.gateway));
}

/// Simulate 8 agents concurrently writing through the **same**
/// `Arc<BibShared>`. All writes must succeed and every row must be
/// readable afterwards. This is the regression test for the
/// per-spawn-reopen issue.
#[tokio::test]
async fn many_concurrent_writers_through_shared_handle() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let bib = shared.bib.clone();

    let mut handles = Vec::new();
    for i in 0..8 {
        let bib = bib.clone();
        handles.push(tokio::spawn(async move {
            let id = format!("agent-{i:02}");
            bib.upsert_article(&sample_article(&id)).await.map(|_| id)
        }));
    }

    let mut saved_ids = Vec::new();
    for h in handles {
        let id = h.await.expect("task panicked").expect("save failed");
        saved_ids.push(id);
    }
    saved_ids.sort();

    // Every article must be reachable through the same shared handle.
    for id in &saved_ids {
        let got = bib.get_article(id).await.expect("get").expect("present");
        assert_eq!(got.id, *id);
    }
}

/// Two agents racing on the same article ID — the upsert is idempotent,
/// so the final state is well-defined regardless of which write wins
/// last. The crucial property is that neither panics, deadlocks, nor
/// leaves the row missing.
#[tokio::test]
async fn racing_writes_same_article_id_succeed() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let bib = shared.bib.clone();

    let b1 = bib.clone();
    let b2 = bib.clone();
    let h1 = tokio::spawn(async move {
        b1.upsert_article(&sample_article("race-01")).await
    });
    let h2 = tokio::spawn(async move {
        b2.upsert_article(&sample_article("race-01")).await
    });

    let r1 = h1.await.unwrap();
    let r2 = h2.await.unwrap();
    // Both writes should succeed.
    assert!(r1.is_ok(), "first write failed: {r1:?}");
    assert!(r2.is_ok(), "second write failed: {r2:?}");

    let got = bib.get_article("race-01").await.unwrap().unwrap();
    assert_eq!(got.id, "race-01");
}
