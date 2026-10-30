//! P0/P1 集成测试:wiki 链接自动解析(P0)、软删除回收站、版本快照、
//! 提及(P1)。全部走真实 TursoWikiStorage。

use wiki::storage::error::Error;
use wiki::storage::turso_wiki_storage::TursoWikiStorage;
use wiki::storage::types::{
    KnowledgeEntryData, WikiEntryStorage, WikiIndexStorage, WikiSearchStorage,
};

fn e(id: uuid::Uuid, title: &str, alias: &[&str], content: &str) -> KnowledgeEntryData {
    KnowledgeEntryData {
        id: id.to_string(),
        title: title.to_string(),
        alias: alias.iter().map(|s| s.to_string()).collect(),
        tags: vec![],
        content: content.to_string(),
        ctime: 1_000,
        mtime: 1_000,
    }
}

async fn storage() -> TursoWikiStorage {
    TursoWikiStorage::open_in_memory().await.unwrap()
}

// ─────────────────────── P0:wiki 链接自动解析 ───────────────────────

#[tokio::test]
async fn wiki_links_resolve_by_title_and_alias() {
    let s = storage().await;
    let genetics = uuid::Uuid::new_v4();
    let methods = uuid::Uuid::new_v4();
    s.create_entry(e(genetics, "遗传学", &["GWAS"], "研究遗传与变异。"))
        .await
        .unwrap();
    s.create_entry(e(
        methods,
        "方法学",
        &[],
        "参见 [[遗传学]] 与 [[GWAS]];另见 [[悬空目标]] 与 [[方法学]] 自身。",
    ))
    .await
    .unwrap();

    // 标题与别名都解析到同一词条;悬空目标不入库;自指边跳过。
    let out = s.get_out_links_batch(&[methods.to_string()]).await.unwrap();
    assert_eq!(out, vec![vec![genetics.to_string()]]);

    // 反向镜像。
    let inn = s.get_in_links_batch(&[genetics.to_string()]).await.unwrap();
    assert_eq!(inn, vec![vec![methods.to_string()]]);
}

#[tokio::test]
async fn update_rewires_links() {
    let s = storage().await;
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let c = uuid::Uuid::new_v4();
    for (id, title) in [(a, "A"), (b, "B"), (c, "C")] {
        s.create_entry(e(id, title, &[], "正文")).await.unwrap();
    }

    let mut eb = e(b, "B", &[], "指向 [[A]]");
    s.update_entry(eb.clone()).await.unwrap();
    let out = s.get_out_links_batch(&[b.to_string()]).await.unwrap();
    assert_eq!(out, vec![vec![a.to_string()]]);

    // 改链后旧边消失,倒排与链接一起换。
    eb.content = "改指 [[C]]".to_string();
    s.update_entry(eb).await.unwrap();
    let out = s.get_out_links_batch(&[b.to_string()]).await.unwrap();
    assert_eq!(out, vec![vec![c.to_string()]]);
    let inn = s.get_in_links_batch(&[a.to_string()]).await.unwrap();
    assert_eq!(inn, vec![Vec::<String>::new()]);
}

// ─────────────────────── P1:软删除回收站 ───────────────────────

#[tokio::test]
async fn soft_delete_hides_and_restore_recovers() {
    let s = storage().await;
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    s.create_entry(e(a, "遗传学", &[], "genome 检索内容"))
        .await
        .unwrap();
    s.create_entry(e(b, "方法学", &[], "指向 [[遗传学]]"))
        .await
        .unwrap();

    s.delete_entry(a).await.unwrap();

    // 读取/检索/邻接全部隐去。
    assert!(matches!(s.get_entry(a).await, Err(Error::NotFound(_))));
    assert!(s.search("genome", 10).await.unwrap().is_empty());
    let out = s.get_out_links_batch(&[b.to_string()]).await.unwrap();
    assert_eq!(out, vec![Vec::<String>::new()]);
    // 已删词条不可更新。
    assert!(matches!(
        s.update_entry(e(a, "遗传学", &[], "改稿")).await,
        Err(Error::NotFound(_))
    ));

    // 回收站可见;标题仍被占用。
    let bin = s.list_deleted().await.unwrap();
    assert_eq!(bin.len(), 1);
    assert_eq!(bin[0].entry.id, a.to_string());
    assert!(bin[0].deleted_at > 0);
    assert!(matches!(
        s.create_entry(e(uuid::Uuid::new_v4(), "遗传学", &[], "新词条"))
            .await,
        Err(Error::DuplicateTitle { .. })
    ));

    // 恢复:索引与链接原样回来。
    s.restore_entry(a).await.unwrap();
    let got = s.get_entry(a).await.unwrap();
    assert_eq!(got.title, "遗传学");
    let out = s.get_out_links_batch(&[b.to_string()]).await.unwrap();
    assert_eq!(out, vec![vec![a.to_string()]]);
    assert_eq!(s.search("genome", 10).await.unwrap().len(), 1);
    assert!(s.list_deleted().await.unwrap().is_empty());
}

#[tokio::test]
async fn purge_frees_title_and_scrubs_history() {
    let s = storage().await;
    let a = uuid::Uuid::new_v4();
    s.create_entry(e(a, "临时", &[], "draft v1")).await.unwrap();
    let ea = e(a, "临时", &[], "draft v2");
    s.update_entry(ea).await.unwrap();
    assert_eq!(s.list_versions(a).await.unwrap().len(), 2);

    s.delete_entry(a).await.unwrap();
    s.purge_entry(a).await.unwrap();

    assert!(s.list_deleted().await.unwrap().is_empty());
    assert!(s.list_versions(a).await.unwrap().is_empty());
    assert!(matches!(s.get_version(a, 1).await, Err(Error::NotFound(_))));
    // 标题释放。
    s.create_entry(e(uuid::Uuid::new_v4(), "临时", &[], "重用标题"))
        .await
        .unwrap();
    // 重复净删幂等。
    s.purge_entry(a).await.unwrap();
}

// ─────────────────────── P1:版本快照 ───────────────────────

#[tokio::test]
async fn versions_snapshot_every_write() {
    let s = storage().await;
    let id = uuid::Uuid::new_v4();
    let mut entry = e(id, "草稿", &[], "第一版内容");
    s.create_entry(entry.clone()).await.unwrap();

    entry.content = "第二版内容".to_string();
    entry.mtime = 2_000;
    s.update_entry(entry).await.unwrap();

    let versions = s.list_versions(id).await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.version).collect::<Vec<_>>(),
        vec![2, 1],
        "新→旧"
    );

    let v1 = s.get_version(id, 1).await.unwrap();
    assert_eq!(v1.content, "第一版内容");
    assert_eq!(v1.tags, Vec::<String>::new());

    // 回滚 = 取快照走普通更新:正文回到 v1,历史追加 v3 不丢。
    let mut rollback = s.get_version(id, 1).await.unwrap();
    rollback.mtime = 3_000;
    s.update_entry(rollback).await.unwrap();
    assert_eq!(s.get_entry(id).await.unwrap().content, "第一版内容");
    assert_eq!(s.list_versions(id).await.unwrap().len(), 3);

    // 不存在的版本/词条。
    assert!(matches!(
        s.get_version(id, 99).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        s.get_version(uuid::Uuid::new_v4(), 1).await,
        Err(Error::NotFound(_))
    ));
}

// ─────────────────────── P1:提及 ───────────────────────

#[tokio::test]
async fn mentions_find_unlinked_plain_text() {
    let s = storage().await;
    let mr = uuid::Uuid::new_v4();
    let plain = uuid::Uuid::new_v4();
    let linked = uuid::Uuid::new_v4();
    let by_alias = uuid::Uuid::new_v4();
    s.create_entry(e(mr, "孟德尔随机化", &["MR"], "定义。"))
        .await
        .unwrap();
    s.create_entry(e(
        plain,
        "应用笔记",
        &[],
        "本文使用孟德尔随机化方法做了因果推断。",
    ))
    .await
    .unwrap();
    s.create_entry(e(linked, "已链接笔记", &[], "按 [[孟德尔随机化]] 的框架。"))
        .await
        .unwrap();
    s.create_entry(e(by_alias, "缩写笔记", &[], "MR 分析的统计功效。"))
        .await
        .unwrap();

    let mentions = s.get_mentions(&mr.to_string()).await.unwrap();
    // 纯文本标题提及与别名提及都命中;已链接的不算提及。
    assert!(mentions.contains(&plain.to_string()));
    assert!(mentions.contains(&by_alias.to_string()));
    assert!(!mentions.contains(&linked.to_string()));

    // 目标不存在 → NotFound。
    assert!(matches!(
        s.get_mentions("no-such-id").await,
        Err(Error::NotFound(_))
    ));
}
