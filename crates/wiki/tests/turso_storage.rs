//! TursoWikiStorage 集成测试:内存库验证语义,文件库验证多进程 WAL
//! 双开(与生产同款打开路径)。

use wiki::storage::error::Error;
use wiki::storage::turso_wiki_storage::TursoWikiStorage;
use wiki::storage::types::{KnowledgeEntryData, WikiEntryStorage, WikiIndexStorage};

fn entry(id: uuid::Uuid, title: &str) -> KnowledgeEntryData {
    KnowledgeEntryData {
        id: id.to_string(),
        title: title.to_string(),
        alias: vec![format!("alias-of-{title}")],
        tags: vec![],
        content: format!("# {title}\n\nbody"),
        ctime: 1_000,
        mtime: 1_000,
    }
}

#[tokio::test]
async fn entry_crud_roundtrip() {
    let storage = TursoWikiStorage::open_in_memory().await.unwrap();
    let id = uuid::Uuid::new_v4();
    let mut e = entry(id, "Home");
    storage.create_entry(e.clone()).await.unwrap();

    let got = storage.get_entry(id).await.unwrap();
    assert_eq!(got.id, e.id);
    assert_eq!(got.title, "Home");
    assert_eq!(got.alias, vec!["alias-of-Home".to_string()]);
    assert_eq!(got.ctime, 1_000);

    e.content = "rewritten".to_string();
    e.mtime = 2_000;
    storage.update_entry(e).await.unwrap();
    let got = storage.get_entry(id).await.unwrap();
    assert_eq!(got.content, "rewritten");
    assert_eq!(got.mtime, 2_000);

    storage.delete_entry(id).await.unwrap();
    assert!(matches!(
        storage.get_entry(id).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn update_missing_entry_is_not_found() {
    let storage = TursoWikiStorage::open_in_memory().await.unwrap();
    let id = uuid::Uuid::new_v4();
    assert!(matches!(
        storage.update_entry(entry(id, "Ghost")).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn duplicate_title_is_rejected_with_occupant() {
    let storage = TursoWikiStorage::open_in_memory().await.unwrap();
    let first = uuid::Uuid::new_v4();
    storage.create_entry(entry(first, "Same")).await.unwrap();

    // 创建撞名:类型化错误携带占用者 id。
    let err = storage
        .create_entry(entry(uuid::Uuid::new_v4(), "Same"))
        .await
        .unwrap_err();
    match err {
        Error::DuplicateTitle { title, existing_id } => {
            assert_eq!(title, "Same");
            assert_eq!(existing_id, first.to_string());
        }
        other => panic!("expected DuplicateTitle, got {other:?}"),
    }

    // 重命名撞名:同样携带占用者 id;重命名为自身现名允许。
    let second = uuid::Uuid::new_v4();
    storage.create_entry(entry(second, "Other")).await.unwrap();
    let err = storage
        .update_entry(entry(first, "Other"))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::DuplicateTitle { .. }));
    storage.update_entry(entry(first, "Same")).await.unwrap();
}

#[tokio::test]
async fn title_is_trimmed_and_whitespace_only_rejected() {
    let storage = TursoWikiStorage::open_in_memory().await.unwrap();
    let id = uuid::Uuid::new_v4();

    // 首尾空白写入时被 trim,读回是干净标题。
    storage.create_entry(entry(id, "  Home  ")).await.unwrap();
    assert_eq!(storage.get_entry(id).await.unwrap().title, "Home");

    // trim 后才判重:空白变体不会再绕出双胞胎。
    assert!(matches!(
        storage
            .create_entry(entry(uuid::Uuid::new_v4(), " Home "))
            .await,
        Err(Error::DuplicateTitle { .. })
    ));

    // 全空白标题直接拒绝;批量里混入同样整批拒绝。
    assert!(matches!(
        storage
            .create_entry(entry(uuid::Uuid::new_v4(), "   "))
            .await,
        Err(Error::EmptyTitle)
    ));
    let good = entry(uuid::Uuid::new_v4(), "Fresh");
    let bad = entry(uuid::Uuid::new_v4(), "\t\n");
    assert!(matches!(
        storage.update_entry_batch(&[good, bad]).await,
        Err(Error::EmptyTitle)
    ));
}

#[tokio::test]
async fn batch_get_preserves_input_order_and_dedups() {
    let storage = TursoWikiStorage::open_in_memory().await.unwrap();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let c = uuid::Uuid::new_v4();
    for (id, title) in [(a, "A"), (b, "B"), (c, "C")] {
        storage.create_entry(entry(id, title)).await.unwrap();
    }

    // 顺序跟随入参;重复 id 只出现一次。
    let got = storage.get_entry_batch(vec![c, a, c]).await.unwrap();
    let titles: Vec<&str> = got.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, vec!["C", "A"]);

    // 未命中的 id 被跳过,不影响其余结果。
    let got = storage
        .get_entry_batch(vec![b, uuid::Uuid::new_v4()])
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].title, "B");

    // 空入参返回空。
    assert!(storage.get_entry_batch(vec![]).await.unwrap().is_empty());
}

#[tokio::test]
async fn batch_update_rolls_back_on_failure() {
    let storage = TursoWikiStorage::open_in_memory().await.unwrap();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let mut ea = entry(a, "A");
    storage.create_entry(ea.clone()).await.unwrap();
    storage.create_entry(entry(b, "B")).await.unwrap();

    // 第一条合法更新,第二条 title 撞占用 → 整批回滚。
    ea.title = "A2".to_string();
    ea.mtime = 5_000;
    let eb = entry(b, "A2");
    assert!(matches!(
        storage.update_entry_batch(&[ea.clone(), eb]).await,
        Err(Error::DuplicateTitle { .. })
    ));

    let got = storage.get_entry(a).await.unwrap();
    assert_eq!(got.title, "A");
    assert_eq!(got.mtime, 1_000);

    // 空批是 no-op。
    storage.update_entry_batch(&[]).await.unwrap();
}

#[tokio::test]
async fn out_links_replace_and_in_links_mirror() {
    let storage = TursoWikiStorage::open_in_memory().await.unwrap();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let c = uuid::Uuid::new_v4();
    for (id, title) in [(a, "A"), (b, "B"), (c, "C")] {
        storage.create_entry(entry(id, title)).await.unwrap();
    }

    // 重复目标去重。
    storage
        .replace_out_links(
            &a.to_string(),
            &[b.to_string(), c.to_string(), b.to_string()],
        )
        .await
        .unwrap();
    storage
        .replace_out_links(&b.to_string(), &[c.to_string()])
        .await
        .unwrap();

    // 桶内按值排序,期望值同样排序后比较。
    let mut expected_a = vec![b.to_string(), c.to_string()];
    expected_a.sort();
    let out = storage
        .get_out_links_batch(&[a.to_string(), b.to_string(), a.to_string()])
        .await
        .unwrap();
    assert_eq!(
        out,
        vec![expected_a.clone(), vec![c.to_string()], expected_a]
    );

    let mut expected_c_in = vec![a.to_string(), b.to_string()];
    expected_c_in.sort();
    let inn = storage
        .get_in_links_batch(&[c.to_string(), a.to_string()])
        .await
        .unwrap();
    assert_eq!(inn, vec![expected_c_in, vec![]]);

    // 覆盖式重写:旧边消失。
    storage
        .replace_out_links(&a.to_string(), &[b.to_string()])
        .await
        .unwrap();
    let out = storage.get_out_links_batch(&[a.to_string()]).await.unwrap();
    assert_eq!(out, vec![vec![b.to_string()]]);

    // 空入参返回空。
    assert!(storage.get_out_links_batch(&[]).await.unwrap().is_empty());
    assert!(storage.get_in_links_batch(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_entry_prunes_links_both_directions() {
    let storage = TursoWikiStorage::open_in_memory().await.unwrap();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    for (id, title) in [(a, "A"), (b, "B")] {
        storage.create_entry(entry(id, title)).await.unwrap();
    }
    storage
        .replace_out_links(&a.to_string(), &[b.to_string()])
        .await
        .unwrap();

    storage.delete_entry(b).await.unwrap();

    // a 的出边随 b 的删除被清掉;删除不存在的 id 是幂等 no-op。
    let out = storage.get_out_links_batch(&[a.to_string()]).await.unwrap();
    assert_eq!(out, vec![Vec::<String>::new()]);
    storage.delete_entry(b).await.unwrap();
}

#[tokio::test]
async fn file_storage_supports_concurrent_handles() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("wiki.db").to_string_lossy().to_string();

    // 两个实例同时持有同一文件(daemon + TUI 的单进程内缩影),schema
    // 幂等初始化跑两遍。
    let writer = TursoWikiStorage::open(&path).await.unwrap();
    let reader = TursoWikiStorage::open(&path).await.unwrap();

    let id = uuid::Uuid::new_v4();
    writer.create_entry(entry(id, "Shared")).await.unwrap();

    let got = reader.get_entry(id).await.unwrap();
    assert_eq!(got.title, "Shared");
    assert_eq!(got.alias, vec!["alias-of-Shared".to_string()]);
}
