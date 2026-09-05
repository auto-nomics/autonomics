//! 内嵌 API 服务装配：RuntimeHost → tui-http router → 随机端口启动。
//!
//! 与 `apps/tui/src/app/terminal.rs::start_http_server` 同一套组装方式
//! （P3，docs/design/web-agent-runtime.md §9）：`RuntimeHost::open` 打开
//! 全套共享基础设施（agent.db / bib.db / vfs / engine），router 经
//! `.host(infra)` 挂上常驻 agent 线程端点。TUI 与桌面版可能同时启动，
//! `state_dir/runtime.lock` 单写者锁保证同一时刻只有一个进程写这组
//! 数据库——拿不到锁在这里直接报错退出。

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use agentik_sdk::model::Model;

use crate::paths::Paths;

pub struct StartedServer {
    pub handle: tui_http::HttpServerHandle,
    /// 进程生命周期存活的 RuntimeHost——持有单写者锁与共享基础设施，
    /// drop 时先于 tokio runtime 才不会悬挂（见 `state::DesktopState`）。
    pub host: runtime::RuntimeHost,
    /// 进程主模型槽：与 router / `host.set_model` 共享同一 Arc。
    pub model_slot: Arc<ArcSwapOption<Model>>,
}

/// 在**当前 tokio runtime** 上启动内嵌服务（`tui_http::start` 会 `bind`
/// 完成后才返回，成功即端口已定）。
pub async fn start_server(paths: &Paths) -> Result<StartedServer, String> {
    let config = runtime_config(paths);

    let mut host = runtime::RuntimeHost::open(&config).await.map_err(|e| match e {
        runtime::Error::InstanceLockHeld { path } => format!(
            "Autonomics 已在 TUI 或另一个桌面实例中运行\n（{} 被占用）。\n\n\
             请先关闭正在运行的实例，再重新打开桌面版。",
            path.display()
        ),
        other => format!("打开 Autonomics 运行时失败：{other}"),
    })?;

    // 模型槽：进程主槽（TUI `App::new` 同款）。config.db 只提供启动时的
    // 初始值；此后 router 与 host 共享这一个 Arc，tui-http 每个 turn 都
    // 会把槽值重新 set_model 进常驻 agent——槽上的任何热替换下一轮生效，
    // 不再存在"启动时读一次、TUI 是唯一写入方"的假设。
    let conn = app_config::open(&paths.app_db)
        .map_err(|e| format!("打开配置库 {} 失败：{e}", paths.app_db.display()))?;
    let model_slot = Arc::new(ArcSwapOption::from_pointee(app_config::build_model(&conn)));
    drop(conn);
    host.set_model(model_slot.clone());

    let infra = host.infra();

    // Settings-saved EasyScholar key survives restarts (the settings PUT
    // hot-swaps only the process that receives it). Load before the router
    // starts serving; `EASYSCHOLAR_KEY` only bootstraps the first run.
    tui_http::load_stored_easyscholar_key(infra.bib.as_ref()).await;

    let bearer = std::env::var("AUTONOMICS_HTTP_API_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty());

    let router = tui_http::ApiRouterBuilder::new(infra.bib.as_ref().clone())
        .bearer_token(bearer)
        .model(model_slot.clone())
        .host(infra)
        .build();

    let handle = tui_http::start(router, ("127.0.0.1", 0))
        .await
        .map_err(|e| format!("绑定 127.0.0.1:0 失败：{e}"))?;

    tracing::info!(url = %handle.url(), "内嵌服务已启动");
    Ok(StartedServer {
        handle,
        host,
        model_slot,
    })
}

/// 从桌面壳路径构造 RuntimeConfig：显式钉住 state/data/bib/app 四个位置
/// （`Paths` 已按同一组 `AUTONOMICS_*` env 解析过，与 TUI 的默认语义一致）。
/// 其余库文件（agent.db / dag-history.db / writing.db）跟随 state_dir，
/// 与 TUI 落到同一组文件——这是 D2"desktop 跟"的前提。
fn runtime_config(paths: &Paths) -> runtime::RuntimeConfig {
    runtime::RuntimeConfig::builder()
        .name("desktop")
        .data_dir(&paths.data_dir)
        .state_dir(&paths.state_dir)
        .bib_db_path(&paths.bib_db)
        .app_db_path(&paths.app_db)
        .build()
}
