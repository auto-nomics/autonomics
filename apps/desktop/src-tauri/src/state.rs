//! 桌面壳生命周期状态：自有 tokio Runtime + 服务句柄，退出时优雅停机。

use agentik_sdk::model::Model;
use std::sync::Arc;
use arc_swap::ArcSwapOption;

pub struct DesktopState {
    /// 自有 runtime（不用 tauri::async_runtime，退出时序完全自控）。
    runtime: Option<tokio::runtime::Runtime>,
    server: Option<tui_http::HttpServerHandle>,
    /// 装配进 router 的模型槽，保活（router 实际持有）并留作热重载入口。
    #[allow(dead_code)] // v2：窗口 Focused 时比对 config.db mtime 触发热换
    pub model_slot: Arc<ArcSwapOption<Model>>,
}

impl DesktopState {
    pub fn new(
        runtime: tokio::runtime::Runtime,
        server: tui_http::HttpServerHandle,
        model_slot: Arc<ArcSwapOption<Model>>,
    ) -> Self {
        Self {
            runtime: Some(runtime),
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

    /// `RunEvent::Exit`：取消 accept loop 并限时等待优雅停机。
    /// 窗口已关 → webview 死 → SSE 连接断开自然收尾；3 秒超时兜底防悬挂。
    pub fn shutdown(&mut self) {
        if let (Some(rt), Some(server)) = (self.runtime.take(), self.server.take()) {
            let _ = rt.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(3), server.shutdown()).await
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
