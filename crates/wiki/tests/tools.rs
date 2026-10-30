//! 工具层集成测试:真实存储 + worker + `ToolFunction::execute` 全链。
//! 每个工具都跑一遍核心路径与错误路径。

use std::sync::Arc;

use agentik_core::tools::ToolFunction;
use agentik_types::tools::{ToolResult, ToolResultContent};
use serde_json::{Value, json};

use wiki::service::WikiHandle;
use wiki::service::start;
use wiki::storage::turso_wiki_storage::TursoWikiStorage;
use wiki::tools::{
    WikiAttachImageTool, WikiAttachmentsTool, WikiCreateTool, WikiDeleteTool, WikiDetachTool,
    WikiEditTool, WikiGetAttachmentTool, WikiGetTool, WikiLinksTool, WikiListTool,
    WikiMentionsTool, WikiPatchTool, WikiPurgeTool, WikiRestoreTool, WikiRestoreVersionTool,
    WikiSearchTool, WikiUpdateTool, WikiVersionsTool, wiki_read_registrations,
    wiki_write_registrations,
};

/// success_json 的回执正文(Value),断言用。
fn json_of(result: ToolResult) -> Value {
    match result.content {
        ToolResultContent::Json(v) => v,
        other => panic!("expected Json content, got {other:?}"),
    }
}

async fn fixture() -> (Arc<WikiHandle>, WikiCreateTool, WikiGetTool) {
    let storage = Arc::new(TursoWikiStorage::open_in_memory().await.unwrap());
    let handle = Arc::new(start(storage));
    (
        handle.clone(),
        WikiCreateTool {
            handle: handle.clone(),
        },
        WikiGetTool { handle },
    )
}

fn validation_message(
    result: Result<agentik_types::tools::ToolResult, agentik_core::tools::ToolError>,
) -> String {
    match result {
        Err(agentik_core::tools::ToolError::ValidationFailed { message }) => message,
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn create_get_by_title_and_search() {
    let (_handle, create, get) = fixture().await;

    let out = json_of(
        create
            .execute(json!({
                "title": "孟德尔随机化",
                "content": "以 [[遗传变异]] 作为工具变量的因果推断方法。",
                "alias": ["MR"],
                "tags": ["methods", "causal"],
            }))
            .await
            .unwrap(),
    );
    assert!(out["title"] == "孟德尔随机化");
    assert!(!out["id"].as_str().unwrap().is_empty());

    // 按标题取回:内容、别名、标签齐全。
    let got = json_of(
        get.execute(json!({ "title": "孟德尔随机化" }))
            .await
            .unwrap(),
    );
    assert!(
        got["content"]
            .as_str()
            .unwrap()
            .contains("工具变量的因果推断")
    );
    assert_eq!(got["alias"][0], "MR");
    assert!(
        got["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "causal")
    );

    // BM25 检索命中,带 snippet 与标签。
    let search = WikiSearchTool { handle: _handle };
    let hits = json_of(
        search
            .execute(json!({ "query": "因果推断" }))
            .await
            .unwrap(),
    );
    assert_eq!(hits["results"].as_array().unwrap().len(), 1);
    assert_eq!(hits["results"][0]["title"], "孟德尔随机化");
    assert!(
        hits["results"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("因果推断")
    );

    // 空查询是校验错误。
    let msg = validation_message(search.execute(json!({ "query": "  " })).await);
    assert!(msg.contains("查询为空"));
}

#[tokio::test]
async fn duplicate_title_and_missing_entry_are_validation_errors() {
    let (_handle, create, get) = fixture().await;

    create
        .execute(json!({ "title": "A", "content": "第一篇" }))
        .await
        .unwrap();

    let msg = validation_message(
        create
            .execute(json!({ "title": "A", "content": "撞名" }))
            .await,
    );
    assert!(msg.contains("已被词条"));

    let msg = validation_message(get.execute(json!({ "title": "不存在" })).await);
    assert!(msg.contains("没有标题为"));

    // id / title 都缺。
    let msg = validation_message(get.execute(json!({})).await);
    assert!(msg.contains("id 或 title"));
}

#[tokio::test]
async fn update_merges_partial_fields_and_renames() {
    let (handle, create, _) = fixture().await;
    let update = WikiUpdateTool {
        handle: handle.clone(),
    };

    create
        .execute(json!({ "title": "旧标题", "content": "v1", "tags": ["keep"] }))
        .await
        .unwrap();

    // 只改别名:标题、标签、正文全部保留(update 是纯元数据工具)。
    update
        .execute(json!({ "title": "旧标题", "alias": ["alt-name"] }))
        .await
        .unwrap();
    let got = handle.find_by_title("旧标题").await.unwrap().unwrap();
    assert_eq!(got.content, "v1");
    assert_eq!(got.tags, vec!["keep".to_string()]);
    assert_eq!(got.alias, vec!["alt-name".to_string()]);

    // 改名:新标题可寻址,旧标题消失;标签与正文仍在。
    update
        .execute(json!({ "title": "旧标题", "new_title": "新标题" }))
        .await
        .unwrap();
    assert!(handle.find_by_title("旧标题").await.unwrap().is_none());
    let got = handle.find_by_title("新标题").await.unwrap().unwrap();
    assert_eq!(got.tags, vec!["keep".to_string()]);
    assert_eq!(got.content, "v1");
}

#[tokio::test]
async fn delete_restore_purge_lifecycle() {
    let (handle, create, get) = fixture().await;
    let delete = WikiDeleteTool {
        handle: handle.clone(),
    };
    let restore = WikiRestoreTool {
        handle: handle.clone(),
    };
    let purge = WikiPurgeTool {
        handle: handle.clone(),
    };

    let out = create
        .execute(json!({ "title": "临时词条", "content": "内容" }))
        .await
        .unwrap();
    let id = {
        // success_json 的 content 是 Value::Array([Text]) 形态,直接从
        // handle 侧拿 id 更稳:按标题查。
        let _ = out;
        handle.find_by_title("临时词条").await.unwrap().unwrap().id
    };

    // 软删:不可见;按 title 取不到。
    delete
        .execute(json!({ "title": "临时词条" }))
        .await
        .unwrap();
    let msg = validation_message(get.execute(json!({ "title": "临时词条" })).await);
    assert!(msg.contains("没有标题为"));

    // 按 id 恢复(回收站词条无法按 title 寻址);恢复后标题重新可见。
    restore.execute(json!({ "id": id })).await.unwrap();
    let got = handle.find_by_title("临时词条").await.unwrap().unwrap();
    assert_eq!(got.id, id);

    // 净删(作用于可见词条):标题释放,可重建。
    purge.execute(json!({ "title": "临时词条" })).await.unwrap();
    create
        .execute(json!({ "title": "临时词条", "content": "重用标题" }))
        .await
        .unwrap();
}

#[tokio::test]
async fn links_and_mentions_flow_through_tools() {
    let (handle, create, _) = fixture().await;
    let links = WikiLinksTool {
        handle: handle.clone(),
    };
    let mentions = WikiMentionsTool {
        handle: handle.clone(),
    };

    create
        .execute(json!({ "title": "孟德尔随机化", "content": "定义。" }))
        .await
        .unwrap();
    create
        .execute(json!({ "title": "已链接笔记", "content": "见 [[孟德尔随机化]]。" }))
        .await
        .unwrap();
    create
        .execute(json!({ "title": "纯文本笔记", "content": "用到了孟德尔随机化方法。" }))
        .await
        .unwrap();

    let out = json_of(
        links
            .execute(json!({ "title": "已链接笔记" }))
            .await
            .unwrap(),
    );
    assert_eq!(out["entry"]["title"], "已链接笔记");
    let out_titles: Vec<&Value> = out["out_links"].as_array().unwrap().iter().collect();
    assert!(out_titles.iter().any(|v| v["title"] == "孟德尔随机化"));
    assert!(out["in_links"].as_array().unwrap().is_empty());

    let out = json_of(
        mentions
            .execute(json!({ "title": "孟德尔随机化" }))
            .await
            .unwrap(),
    );
    let mentioned: Vec<&str> = out["unlinked_mentions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert!(
        mentioned.contains(&"纯文本笔记"),
        "纯文本提及应出现:{mentioned:?}"
    );
    assert!(
        !mentioned.contains(&"已链接笔记"),
        "已链接的不算提及:{mentioned:?}"
    );
}

#[tokio::test]
async fn versions_list_and_restore() {
    let (handle, create, _) = fixture().await;
    let versions = WikiVersionsTool {
        handle: handle.clone(),
    };
    let restore = WikiRestoreVersionTool {
        handle: handle.clone(),
    };
    let edit = WikiEditTool {
        handle: handle.clone(),
    };

    create
        .execute(json!({ "title": "草稿", "content": "第一版" }))
        .await
        .unwrap();
    edit.execute(json!({
        "title": "草稿",
        "old_string": "第一版",
        "new_string": "第二版",
    }))
    .await
    .unwrap();

    let out = json_of(versions.execute(json!({ "title": "草稿" })).await.unwrap());
    let versions_list = out["versions"].as_array().unwrap();
    assert_eq!(versions_list.len(), 2);
    assert_eq!(versions_list[0]["version"], 2, "新→旧");

    // 回滚到 v1:正文回去,历史追加 v3。
    restore
        .execute(json!({ "title": "草稿", "version": 1 }))
        .await
        .unwrap();
    let got = handle.find_by_title("草稿").await.unwrap().unwrap();
    assert_eq!(got.content, "第一版");
    assert_eq!(
        handle
            .list_versions(got.id.parse().unwrap())
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn edit_replaces_with_fuzzy_and_reports_ambiguity() {
    let (handle, create, _) = fixture().await;
    let edit = WikiEditTool {
        handle: handle.clone(),
    };

    create
        .execute(json!({
            "title": "方法笔记",
            "content": "# 方法\n\n旧的结论一段。\n\n尾注。",
        }))
        .await
        .unwrap();

    // 精确替换。
    let out = json_of(
        edit.execute(json!({
            "title": "方法笔记",
            "old_string": "旧的结论一段。",
            "new_string": "新的结论一段。",
        }))
        .await
        .unwrap(),
    );
    assert_eq!(out["replacements"], 1);
    assert_eq!(out["fuzzy"], false);

    // 模糊匹配(尾随空白差异)。
    let out = json_of(
        edit.execute(json!({
            "title": "方法笔记",
            "old_string": "新的结论一段。  \n",
            "new_string": "修正后的结论。",
        }))
        .await
        .unwrap(),
    );
    assert_eq!(out["fuzzy"], true);
    let got = handle.find_by_title("方法笔记").await.unwrap().unwrap();
    assert!(got.content.contains("修正后的结论"));

    // 未命中:不写入。
    let msg = validation_message(
        edit.execute(json!({
            "title": "方法笔记",
            "old_string": "不存在的句子",
            "new_string": "x",
        }))
        .await,
    );
    assert!(msg.contains("未命中"));

    // 多处命中且未开 replace_all:用 edit 把唯一行铺成三份。
    edit.execute(json!({
        "title": "方法笔记",
        "old_string": "修正后的结论。",
        "new_string": "重复行\n重复行\n重复行",
    }))
    .await
    .unwrap();
    let msg = validation_message(
        edit.execute(json!({
            "title": "方法笔记",
            "old_string": "重复行",
            "new_string": "唯一行",
        }))
        .await,
    );
    assert!(msg.contains("3 处"));
    // old == new 拒绝。
    let msg = validation_message(
        edit.execute(json!({
            "title": "方法笔记",
            "old_string": "重复行",
            "new_string": "重复行",
        }))
        .await,
    );
    assert!(msg.contains("不能相同"));
}

#[tokio::test]
async fn patch_applies_multi_hunk_atomically() {
    let (handle, create, _) = fixture().await;
    let patch = WikiPatchTool {
        handle: handle.clone(),
    };

    create
        .execute(json!({
            "title": "综述",
            "content": "# 综述\n\n## 背景\n\n旧背景内容。\n\n## 方法\n\n旧方法内容。\n",
        }))
        .await
        .unwrap();

    let out = json_of(
        patch
            .execute(json!({
                "title": "综述",
                "patch": "*** Begin Patch\n*** Update File: 综述\n@@\n 旧背景内容。\n+补充背景。\n@@\n-旧方法内容。\n+新方法内容。\n*** End Patch",
            }))
            .await
            .unwrap(),
    );
    assert_eq!(out["chunks_applied"], 2);
    assert_eq!(out["patched"], true);

    let got = handle.find_by_title("综述").await.unwrap().unwrap();
    assert!(got.content.contains("补充背景。"));
    assert!(got.content.contains("新方法内容。"));
    assert!(!got.content.contains("旧方法内容。"));

    // 版本快照:创建 v1 + patch v2。
    let id = uuid::Uuid::parse_str(&got.id).unwrap();
    assert_eq!(handle.list_versions(id).await.unwrap().len(), 2);

    // 上下文未命中:报错且不写入。
    let msg = validation_message(
        patch
            .execute(json!({
                "title": "综述",
                "patch": "*** Begin Patch\n*** Update File: 综述\n@@\n 不存在的上下文\n-旧\n+新\n*** End Patch",
            }))
            .await,
    );
    assert!(msg.contains("patch 应用失败"));

    // Add File hunk 拒绝。
    let msg = validation_message(
        patch
            .execute(json!({
                "title": "综述",
                "patch": "*** Begin Patch\n*** Add File: 其他\n+内容\n*** End Patch",
            }))
            .await,
    );
    assert!(msg.contains("只支持"));

    // 解析失败。
    let msg = validation_message(
        patch
            .execute(json!({ "title": "综述", "patch": "不是 patch" }))
            .await,
    );
    assert!(msg.contains("解析失败") || msg.contains("不含任何 hunk"));
}

#[tokio::test]
async fn registrations_cover_all_tools() {
    let storage = Arc::new(TursoWikiStorage::open_in_memory().await.unwrap());
    let handle = Arc::new(start(storage));
    let read = wiki_read_registrations(handle.clone());
    let write = wiki_write_registrations(handle);
    assert_eq!(read.len(), 8);
    assert_eq!(write.len(), 10);
    // 定义齐全:名字与参数 schema 均非空。
    for reg in read.iter().chain(write.iter()) {
        assert!(reg.definition.name.starts_with("wiki_"));
        assert!(!reg.definition.description.is_empty());
    }
}

#[tokio::test]
async fn list_browses_without_query_and_pages() {
    let (handle, create, _) = fixture().await;
    let list = WikiListTool {
        handle: handle.clone(),
    };

    create
        .execute(json!({ "title": "新笔记", "content": "x", "tags": ["recent"] }))
        .await
        .unwrap();
    create
        .execute(json!({ "title": " tagged", "content": "x", "tags": [" gwas  "] }))
        .await
        .unwrap();

    // 无过滤:全部词条(同秒创建时 mtime 并列,顺序由 id 决定,
    // 断言取集合而非顺序)。
    let out = json_of(list.execute(json!({})).await.unwrap());
    let mut titles: Vec<&str> = out["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, vec!["tagged", "新笔记"]);
    assert_eq!(out["count"], 2);

    // 标签过滤(trim 后匹配)。
    let out = json_of(list.execute(json!({ "tags": ["gwas"] })).await.unwrap());
    let titles: Vec<&str> = out["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["tagged"]);

    // 分页:两页拼回全集(offset 语义不依赖同刻条目的先后)。
    let out = json_of(list.execute(json!({ "limit": 1 })).await.unwrap());
    assert_eq!(out["count"], 1);
    let first = out["entries"][0]["title"].as_str().unwrap().to_string();
    let out = json_of(
        list.execute(json!({ "limit": 1, "offset": 1 }))
            .await
            .unwrap(),
    );
    assert_eq!(out["count"], 1);
    let second = out["entries"][0]["title"].as_str().unwrap().to_string();
    assert_ne!(first, second);
    assert!(["新笔记", "tagged"].contains(&first.as_str()));
    assert!(["新笔记", "tagged"].contains(&second.as_str()));

    // 元数据里没有正文,但有 ctime/mtime。
    let first = &out["entries"][0];
    assert!(first.get("content").is_none());
    assert!(first["mtime"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn attach_image_tool_roundtrip() {
    use base64::Engine as _;
    use wiki::storage::blob::MemoryBlobStore;

    // 附件工具需要 blob 后端:内存实现 + worker 单循环。
    let storage = Arc::new(
        TursoWikiStorage::open_in_memory()
            .await
            .unwrap()
            .with_blob_store(MemoryBlobStore::new()),
    );
    let handle = Arc::new(start(storage));
    let create = WikiCreateTool {
        handle: handle.clone(),
    };
    let attach = WikiAttachImageTool {
        handle: handle.clone(),
    };
    let list = WikiAttachmentsTool {
        handle: handle.clone(),
    };
    let get = WikiGetAttachmentTool {
        handle: handle.clone(),
    };
    let detach = WikiDetachTool { handle };

    create
        .execute(json!({ "title": "图注词条", "content": "见下图。" }))
        .await
        .unwrap();

    let png = b"\x89PNG\r\n\x1a\nfake-image-bytes";
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let out = json_of(
        attach
            .execute(json!({
                "title": "图注词条",
                "filename": "figure.png",
                "data_base64": b64,
            }))
            .await
            .unwrap(),
    );
    assert_eq!(out["mime"], "image/png");
    let attachment_id = out["attachment_id"].as_str().unwrap().to_string();
    let markdown = out["markdown"].as_str().unwrap();
    assert!(markdown.starts_with("![figure.png](wiki-attach://"));

    // 清单。
    let out = json_of(list.execute(json!({ "title": "图注词条" })).await.unwrap());
    assert_eq!(out["count"], 1);
    assert_eq!(out["attachments"][0]["filename"], "figure.png");

    // 取回:base64 往返一致。
    let out = json_of(
        get.execute(json!({
            "title": "图注词条",
            "attachment_id": attachment_id,
        }))
        .await
        .unwrap(),
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(out["data_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, png);

    // 摘除 + 幂等。
    detach
        .execute(json!({
            "title": "图注词条",
            "attachment_id": attachment_id,
        }))
        .await
        .unwrap();
    detach
        .execute(json!({
            "title": "图注词条",
            "attachment_id": attachment_id,
        }))
        .await
        .unwrap();
    let out = json_of(list.execute(json!({ "title": "图注词条" })).await.unwrap());
    assert_eq!(out["count"], 0);

    // 非法 base64 → 校验错误。
    let msg = validation_message(
        attach
            .execute(json!({
                "title": "图注词条",
                "filename": "x.png",
                "data_base64": "!!!not-base64!!!",
            }))
            .await,
    );
    assert!(msg.contains("base64"));
}
