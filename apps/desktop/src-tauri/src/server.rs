//! 内嵌 API 服务装配：BibShared + 模型槽 → tui-http router → 随机端口启动。
//!
//! 与 `apps/tui/src/app/terminal.rs::start_http_server`（P4 接线）同一套
//! 组装方式，只是模型槽改为桌面壳自己从共享 config.db 解析。

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use agentik_sdk::model::Model;

use crate::paths::Paths;
use crate::vfs_setup;

pub struct StartedServer {
    pub handle: tui_http::HttpServerHandle,
    /// 装配进 router 的活动模型槽；保留引用以便后续热重载（v2）。
    pub model_slot: Arc<ArcSwapOption<Model>>,
}

/// 在**当前 tokio runtime** 上启动内嵌服务（`tui_http::start` 会 `bind`
/// 完成后才返回，成功即端口已定）。
pub async fn start_server(paths: &Paths) -> Result<StartedServer, String> {
    // file_storage 是必需项：None 时全文/文件端点直接报错（bib.rs:1303/2106）。
    let storage = vfs_setup::build_file_storage(paths)?;

    let shared = bib_base::BibShared::open_with(&paths.bib_db, bib_base::BibHttpOptions::default())
        .await
        .map_err(|e| format!("打开文献库 {} 失败：{e}", paths.bib_db.display()))?
        .with_file_storage(storage);

    // 模型槽：启动时读一次共享 config.db（TUI 是唯一写入方）。
    // 无模型 → 槽为 None → agent 路由 503 "no model configured"。
    let conn = app_config::open(&paths.app_db)
        .map_err(|e| format!("打开配置库 {} 失败：{e}", paths.app_db.display()))?;
    let model_slot = Arc::new(ArcSwapOption::from_pointee(app_config::build_model(&conn)));
    drop(conn);

    let bearer = std::env::var("AUTONOMICS_HTTP_API_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty());

    let router = tui_http::ApiRouterBuilder::new(shared)
        .bearer_token(bearer)
        .model(model_slot.clone())
        .build();

    let handle = tui_http::start(router, ("127.0.0.1", 0))
        .await
        .map_err(|e| format!("绑定 127.0.0.1:0 失败：{e}"))?;

    tracing::info!(url = %handle.url(), "内嵌服务已启动");
    Ok(StartedServer {
        handle,
        model_slot,
    })
}
