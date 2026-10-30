//! BM25 全文检索集成测试:真实 TursoWikiStorage 倒排 + jieba 中英分词
//! + BM25Okapi 打分。

use std::sync::Arc;

use wiki::service::start;
use wiki::storage::turso_wiki_storage::TursoWikiStorage;
use wiki::storage::types::{KnowledgeEntryData, WikiEntryStorage, WikiSearchStorage};

fn entry(id: uuid::Uuid, title: &str, alias: &[&str], content: &str) -> KnowledgeEntryData {
    KnowledgeEntryData {
        id: id.to_string(),
        title: title.to_string(),
        alias: alias.iter().map(|s| s.to_string()).collect(),
        content: content.to_string(),
        ctime: 1_000,
        mtime: 1_000,
    }
}

async fn storage() -> TursoWikiStorage {
    TursoWikiStorage::open_in_memory().await.unwrap()
}

#[tokio::test]
async fn english_search_ranks_relevant_entry_first() {
    let s = storage().await;
    let gwas = uuid::Uuid::new_v4();
    let lab = uuid::Uuid::new_v4();
    let reg = uuid::Uuid::new_v4();
    s.create_entry(entry(
        gwas,
        "GWAS 方法论",
        &[],
        "genome wide association study of cohort data",
    ))
    .await
    .unwrap();
    s.create_entry(entry(
        lab,
        "Lab notebook",
        &[],
        "protocol for sample preparation",
    ))
    .await
    .unwrap();
    s.create_entry(entry(
        reg,
        "Regression notes",
        &[],
        "linear model assumptions",
    ))
    .await
    .unwrap();

    let hits = s.search("association study", 10).await.unwrap();
    assert_eq!(hits.len(), 1, "只有 GWAS 条目含这两个词");
    assert_eq!(hits[0].entry_id, gwas.to_string());
}

#[tokio::test]
async fn chinese_search_matches_via_jieba() {
    let s = storage().await;
    let genetics = uuid::Uuid::new_v4();
    let ml = uuid::Uuid::new_v4();
    s.create_entry(entry(
        genetics,
        "遗传学笔记",
        &[],
        "全基因组关联分析用于研究慢性病的遗传风险",
    ))
    .await
    .unwrap();
    s.create_entry(entry(ml, "机器学习", &[], "模型训练与超参数调优"))
        .await
        .unwrap();

    // 查询词是文档长词的子词:靠 cut_for_search 的词典子词命中。
    let hits = s.search("基因组", 10).await.unwrap();
    assert!(!hits.is_empty(), "「基因组」应命中遗传学条目");
    assert_eq!(hits[0].entry_id, genetics.to_string());

    let hits = s.search("关联分析", 10).await.unwrap();
    assert_eq!(hits[0].entry_id, genetics.to_string());

    let hits = s.search("超参数", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_id, ml.to_string());
}

#[tokio::test]
async fn mixed_language_query() {
    let s = storage().await;
    let target = uuid::Uuid::new_v4();
    let other = uuid::Uuid::new_v4();
    s.create_entry(entry(
        target,
        "多组学整合",
        &[],
        "single cell sequencing 单细胞转录组数据分析",
    ))
    .await
    .unwrap();
    s.create_entry(entry(
        other,
        "Statistics",
        &[],
        "p values and confidence intervals",
    ))
    .await
    .unwrap();

    let hits = s.search("单细胞 sequencing", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_id, target.to_string());
}

#[tokio::test]
async fn rare_terms_outrank_common_ones() {
    let s = storage().await;
    let focused = uuid::Uuid::new_v4();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    // 三个条目都含 cohort;只有 focused 含 polygenic。
    for (id, title) in [(focused, "Focused"), (a, "Alpha"), (b, "Beta")] {
        let extra = if id == focused {
            " polygenic score"
        } else {
            ""
        };
        s.create_entry(entry(id, title, &[], &format!("cohort description{extra}")))
            .await
            .unwrap();
    }

    let hits = s.search("cohort polygenic", 10).await.unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(
        hits[0].entry_id,
        focused.to_string(),
        "稀有词 polygenic 应置顶"
    );
    // 得分严格降序。
    for pair in hits.windows(2) {
        assert!(pair[0].score >= pair[1].score);
    }
}

#[tokio::test]
async fn shorter_doc_with_higher_tf_ranks_first() {
    let s = storage().await;
    let dense = uuid::Uuid::new_v4();
    let sparse = uuid::Uuid::new_v4();
    let filler = "background material context review article text ".repeat(40);
    s.create_entry(entry(dense, "Dense", &[], "snp snp snp summary"))
        .await
        .unwrap();
    s.create_entry(entry(
        sparse,
        "Sparse",
        &[],
        &format!("snp mention {filler}"),
    ))
    .await
    .unwrap();

    let hits = s.search("snp", 10).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].entry_id, dense.to_string());
}

#[tokio::test]
async fn alias_is_searchable() {
    let s = storage().await;
    let id = uuid::Uuid::new_v4();
    s.create_entry(entry(
        id,
        "研究笔记一",
        &["GWAS", "gwas-2024"],
        "正文不含关键词",
    ))
    .await
    .unwrap();

    // 大小写折叠:alias 大写、查询小写。
    let hits = s.search("gwas", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_id, id.to_string());
}

#[tokio::test]
async fn limit_truncates_and_empty_query_returns_nothing() {
    let s = storage().await;
    for i in 0..3 {
        s.create_entry(entry(
            uuid::Uuid::new_v4(),
            &format!("Note {i}"),
            &[],
            "shared keyword discussion",
        ))
        .await
        .unwrap();
    }

    assert_eq!(s.search("keyword", 2).await.unwrap().len(), 2);
    assert!(s.search("", 10).await.unwrap().is_empty());
    assert!(s.search("   ", 10).await.unwrap().is_empty());
    assert!(s.search("!!!", 10).await.unwrap().is_empty());
    assert!(s.search("nonexistent-term", 10).await.unwrap().is_empty());
    assert!(s.search("keyword", 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn update_reindexes_entry() {
    let s = storage().await;
    let id = uuid::Uuid::new_v4();
    let mut e = entry(id, "Draft", &[], "alpha beta");
    s.create_entry(e.clone()).await.unwrap();

    let hits = s.search("alpha", 10).await.unwrap();
    assert_eq!(hits.len(), 1);

    e.content = "gamma delta".to_string();
    s.update_entry(e).await.unwrap();

    assert!(
        s.search("alpha", 10).await.unwrap().is_empty(),
        "旧内容应出索引"
    );
    let hits = s.search("gamma", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_id, id.to_string());
}

#[tokio::test]
async fn delete_removes_entry_from_index() {
    let s = storage().await;
    let id = uuid::Uuid::new_v4();
    s.create_entry(entry(id, "Temp", &[], "ephemeral notes"))
        .await
        .unwrap();
    assert_eq!(s.search("ephemeral", 10).await.unwrap().len(), 1);

    s.delete_entry(id).await.unwrap();
    assert!(s.search("ephemeral", 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn worker_search_roundtrip() {
    let s = Arc::new(storage().await);
    let id = uuid::Uuid::new_v4();
    s.create_entry(entry(id, "队列研究", &[], "cohort study 设计与偏倚控制"))
        .await
        .unwrap();

    let handle = start(s);
    let hits = handle.search("cohort 队列", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_id, id.to_string());
}
