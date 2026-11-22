//! 文献引用边(pandoc `[@citekey]`)的解析 → 索引 → 双向查询闭环。

use std::sync::Arc;

use agentik_core::tools::ToolFunction;
use serde_json::{Value, json};
use wiki::service::{WikiHandle, start};
use wiki::storage::turso_wiki_storage::TursoWikiStorage;
use wiki::tools::{WikiCreateTool, WikiEditTool};

async fn handle() -> Arc<WikiHandle> {
    let storage = Arc::new(TursoWikiStorage::open_in_memory().await.unwrap());
    Arc::new(start(storage))
}

fn json_of(result: agentik_types::tools::ToolResult) -> Value {
    match result.content {
        agentik_types::tools::ToolResultContent::Json(v) => v,
        other => panic!("expected Json content, got {other:?}"),
    }
}

async fn create(h: &Arc<WikiHandle>, title: &str, content: &str) -> String {
    let tool = WikiCreateTool {
        handle: h.clone(),
    };
    let out = json_of(
        tool.execute(json!({
            "title": title,
            "content": content,
        }))
        .await
        .unwrap(),
    );
    out["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn citations_indexed_and_queryable_both_ways() {
    let h = handle().await;
    let entry = create(
        &h,
        "LDL 与冠心病",
        "风险结论见 [@smith2024a-k3x9];机制综述 [@jones2021.ab]。另见 [[孟德尔随机化]]。",
    )
    .await;
    let other = create(&h, "第二篇", "复述 [@smith2024a-k3x9]。").await;

    // 正向:词条引用了哪些键(去重,字典序)。
    let keys = h.get_citations(&entry).await.unwrap();
    assert_eq!(keys, vec!["jones2021.ab", "smith2024a-k3x9"]);

    // 反向:谁引用了这个键。
    let citing = h.get_citing_entries("smith2024a-k3x9").await.unwrap();
    assert!(citing.contains(&entry));
    assert!(citing.contains(&other));

    // 悬空键(文献未导入)反查为空。
    assert!(h.get_citing_entries("nobody2099unknown").await.unwrap().is_empty());
}

#[tokio::test]
async fn edit_updates_citation_index() {
    let h = handle().await;
    let id = create(&h, "初始", "引用 [@old2020key]。").await;
    assert_eq!(h.get_citations(&id).await.unwrap(), vec!["old2020key"]);

    let edit = WikiEditTool {
        handle: h.clone(),
    };
    json_of(
        edit.execute(json!({
            "id": id,
            "old_string": "引用 [@old2020key]。",
            "new_string": "改引 [@new2021key]。",
        }))
        .await
        .unwrap(),
    );
    assert_eq!(h.get_citations(&id).await.unwrap(), vec!["new2021key"]);
    assert!(h.get_citing_entries("old2020key").await.unwrap().is_empty());
}

#[tokio::test]
async fn code_block_citations_not_indexed() {
    let h = handle().await;
    let id = create(
        &h,
        "示例",
        "```\n[@incode2020xx]\n```\n正文 [@real2020yy]",
    )
    .await;
    assert_eq!(h.get_citations(&id).await.unwrap(), vec!["real2020yy"]);
}
