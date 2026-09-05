//! 桌面壳生命周期状态：自有 tokio Runtime + RuntimeHost + 服务句柄，退出时优雅停机。

use agentik_sdk::model::Model;
use std::sync::Arc;
use arc_swap::ArcSwapOption;

pub struct DesktopState {
    /// 自有 runtime（不用 tauri::async_runtime，退出时序完全自控）。
    runtime: Option<tokio::runtime::Runtime>,
    /// 进程级 RuntimeHost：持有 `state_dir/runtime.lock` 单写者锁与共享
    /// 基础设施。必须活到停机最后（drop = 释放锁），且先于 tokio runtime
    /// drop——host 收 agent 需要在 runtime 上 block_on。
    host: Option<runtime::RuntimeHost>,
    server: Option<tui_http::HttpServerHandle>,
    /// 进程主模型槽（与 router / host 共享同一 Arc），留作热重载入口。
    #[allow(dead_code)] // v2：设置面板直改模型时的热换入口
    pub model_slot: Arc<ArcSwapOption<Model>>,
}

impl DesktopState {
    pub fn new(
        runtime: tokio::runtime::Runtime,
        host: runtime::RuntimeHost,
        server: tui_http::HttpServerHandle,
        model_slot: Arc<ArcSwapOption<Model>>,
    ) -> Self {
        Self {
            runtime: Some(runtime),
            host: Some(host),
            server: Some(server),
            model_slot,
        }
    }

    pub fn server_url(&self) -> String {
        self.server
            .as_ref()
            .map(|s| s.url())
            .unwrap_or_else(|| "http://127.0.0.1/".to_string())
    }

    /// `RunEvent::Exit`：与 TUI 退出序一致——先停 HTTP（窗口已关 → webview
    /// 死 → SSE 连接断开自然收尾，3 秒超时兜底），再优雅收 agent（暂停
    /// session、落快照、刷 WAL，5 秒超时防悬挂），最后 drop host 释放单写者锁。
    pub fn shutdown(&mut self) {
        if let (Some(rt), Some(server)) = (self.runtime.take(), self.server.take()) {
            let _ = rt.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(3), server.shutdown()).await
            });
        }
        if let (Some(rt), Some(mut host)) = (self.runtime.take(), self.host.take()) {
            let _ = rt.block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    host.shutdown_all_agents_and_wait(),
                )
                .await
            });
        }
    }
}

impl Drop for DesktopState {
    fn drop(&mut self) {
        // 兜底：未走 RunEvent::Exit 的异常路径也不悬挂。
        self.shutdown();
    }
}
