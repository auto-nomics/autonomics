#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gateway;

fn main() {
    #[cfg(target_os = "linux")]
    {
        // NVIDIA + WebKitGTK can present an empty compositor layer unless
        // legacy rendering is forced. Operators can still override either.
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            unsafe { std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1") };
        }
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            gateway::probe,
            gateway::api,
            gateway::start_events,
            gateway::frontend_ready,
            gateway::upload_bib_fulltext,
            gateway::save_bib_export
        ])
        .setup(|app| {
            gateway::install(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Autonomics GUI");
}
