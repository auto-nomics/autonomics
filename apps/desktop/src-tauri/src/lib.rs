//! Autonomics 桌面壳：进程内嵌 tui-http，webview 导航至同源地址。
//!
//! 前端零改动——SPA 由 rust-embed 嵌入服务同源托管，`isTauri` 保持
//! false，agent SSE 裸 fetch 天然工作。Tauri 在这里只提供窗口、
//! 单实例、剪贴板与错误对话框。

mod paths;
mod server;
mod state;
mod vfs_setup;

use std::sync::Mutex;

use tauri::Manager;

use paths::Paths;
use state::DesktopState;

pub fn run() {
    // 路径解析在最前：日志目录也依赖它。失败即无可救药的启动错误。
    let paths = match paths::resolve() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("autonomics-desktop: {e}");
            simple_error_dialog_and_exit(e);
        }
    };
    if let Err(e) = paths.ensure_dirs() {
        eprintln!("autonomics-desktop: {e}");
        simple_error_dialog_and_exit(e);
    }
    init_logging(&paths);

    tauri::Builder::default()
        // 单实例必须最先注册：二次启动聚焦既有窗口后本次进程退出。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        // 前端三处 `__TAURI_INTERNALS__` 剪贴板分支在 webview 内为真，
        // 注册插件 + remote capability（capabilities/default.json）放行。
        .plugin(tauri_plugin_clipboard_manager::init())
        // 仅用于启动失败的原生错误弹窗。
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(Option::<DesktopState>::None))
        .setup(move |app| {
            let desktop = match bootstrap(&paths) {
                Ok(d) => d,
                Err(err) => {
                    tracing::error!(error = %err, "启动失败");
                    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
                    app.dialog()
                        .message(format!("Autonomics 后端初始化失败：\n\n{err}"))
                        .title("Autonomics 启动失败")
                        .kind(MessageDialogKind::Error)
                        .blocking_show();
                    std::process::exit(1);
                }
            };
            let url = tauri::Url::parse(&desktop.server_url())
                .expect("server url 合法（来自 HttpServerHandle::url）");

            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
                .title("Autonomics")
                .inner_size(1440.0, 900.0)
                .min_inner_size(960.0, 640.0)
                .resizable(true)
                .center()
                // 对齐 jayread 桌面外观：无边框（无原生标题栏），
                // 平铺 WM（Hyprland）下移窗/关窗由 WM 键位承担。
                .decorations(false)
                // 禁 Ctrl+滚轮缩放，避免 UI 比例被意外改动。
                .zoom_hotkeys_enabled(false)
                .build()?;

            *app.state::<Mutex<Option<DesktopState>>>().lock().unwrap() = Some(desktop);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(mut desktop) =
                    app.state::<Mutex<Option<DesktopState>>>().lock().unwrap().take()
                {
                    desktop.shutdown();
                }
            }
        });
}

/// 组装内嵌服务。顺序严格：VFS → BibShared → 模型槽 → router → start。
/// `tui_http::start` bind 完成端口即定，窗口最后创建——webview 首次
/// 导航时服务已可用，无需重试逻辑。
fn bootstrap(paths: &Paths) -> Result<DesktopState, String> {
    let runtime =
        tokio::runtime::Runtime::new().map_err(|e| format!("创建 tokio runtime 失败：{e}"))?;

    let started = runtime.block_on(server::start_server(paths))?;

    Ok(DesktopState::new(
        runtime,
        started.handle,
        started.model_slot,
    ))
}

/// 日志：GUI 进程 stderr 无人看，滚动文件 `<state_dir>/logs/desktop.log`；
/// debug 构建额外输出 stderr 便于 `tauri dev` 观察。初始化失败降级为仅
/// stderr，不致命。
fn init_logging(paths: &Paths) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let file_appender = tracing_appender::rolling::daily(&paths.logs_dir, "desktop.log");
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
    // _guard 必须存活于整个进程生命周期；泄漏它（进程级单例）是标准做法。
    std::mem::forget(_guard);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    #[cfg(debug_assertions)]
    {
        let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stderr_layer)
            .init();
    }
    #[cfg(not(debug_assertions))]
    {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .init();
    }
}

/// 日志未就绪时的极简错误路径：调用方已 eprintln，这里直接退出。
/// （tauri 尚未构建，无法弹原生对话框；终端启动时 stderr 可见原因。）
fn simple_error_dialog_and_exit(_msg: String) -> ! {
    std::process::exit(1)
}
