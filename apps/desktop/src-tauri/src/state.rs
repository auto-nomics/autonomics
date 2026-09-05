//! 桌面壳生命周期状态：自有 tokio Runtime + RuntimeHost + 服务句柄，退出时优雅停机。

use agentik_sdk::model::Model;
use std::sync::Arc;
use arc_swap::ArcSwapOption;

pub struct DesktopState {
    /// 自有 runtime（不用 tauri::async_runtime，退出时序完全自控）。
    runtime: Option<tokio::runtime::Runtime>,
    /// 进程级 RuntimeHost 的后台驱动（P5a 起桌面壳无事件循环，`HostControl`
    /// 命令由驱动处理）。join() 收回 host 后按原退出序优雅停机。host 持有
    /// `state_dir/runtime.lock` 单写者锁与共享基础设施，必须活到停机最后
    /// （drop = 释放锁），且先于 tokio runtime drop——host 收 agent 需要在
    /// runtime 上 block_on。
    host_driver: Option<runtime::HostDriver>,
    server: Option<tui_http::HttpServerHandle>,
    /// 进程主模型槽（与 router / host 共享同一 Arc），留作热重载入口。
    #[allow(dead_code)] // v2：设置面板直改模型时的热换入口
    pub model_slot: Arc<ArcSwapOption<Model>>,
}

impl DesktopState {
    pub fn new(
        runtime: tokio::runtime::Runtime,
        host_driver: runtime::HostDriver,
        server: tui_http::HttpServerHandle,
        model_slot: Arc<ArcSwapOption<Model>>,
    ) -> Self {
        Self {
            runtime: Some(runtime),
            host_driver: Some(host_driver),
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
    /// 死 → SSE 连接断开自然收尾，3 秒超时兜底），再停驱动循环收回 host，
    /// 优雅收 agent（暂停 session、落快照、刷 WAL，5 秒超时防悬挂），最后
    /// drop host 释放单写者锁、drop runtime。
    ///
    /// 一个 runtime 串起整个退出序（此前第二个 `take` 永远拿到 None——
    /// 第一个 if-let 已把 runtime move 走，agent 优雅停机从未执行过）。
    pub fn shutdown(&mut self) {
        let Some(rt) = self.runtime.take() else { return };
        if let Some(server) = self.server.take() {
            let _ = rt.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(3), server.shutdown()).await
            });
        }
        if let Some(driver) = self.host_driver.take() {
            let _ = rt.block_on(async {
                if let Ok(mut host) =
                    tokio::time::timeout(std::time::Duration::from_secs(2), driver.join()).await
                {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        host.shutdown_all_agents_and_wait(),
                    )
                    .await;
                }
            });
        }
        // rt 在此 drop——晚于 host（锁释放）与 server。
    }
}

impl Drop for DesktopState {
    fn drop(&mut self) {
        // 兜底：未走 RunEvent::Exit 的异常路径也不悬挂。
        self.shutdown();
    }
}
