//! End-to-end wiki image qualification in a real Tauri WebView.
#[allow(dead_code)]
#[path = "../src/gateway.rs"]
mod gateway;
#[allow(dead_code)]
#[path = "../src/request_cancellation.rs"]
mod request_cancellation;
use base64::Engine as _;
use std::{sync::Arc, time::Duration};
use tauri::Manager;

#[tauri::command]
fn smoke_done(app: tauri::AppHandle, result: serde_json::Value) {
    let output = std::env::var("WIKI_NATIVE_SMOKE_OUTPUT")
        .unwrap_or_else(|_| "/tmp/wiki-native-smoke.json".into());
    std::fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("native wiki qualification: {result}");
    app.exit(if result["ok"] == true { 0 } else { 1 });
}
fn main() {
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(async {
        let storage = wiki::storage::turso_wiki_storage::TursoWikiStorage::open_in_memory().await.unwrap()
            .with_blob_store(wiki::storage::blob::MemoryBlobStore::new());
        let handle = Arc::new(wiki::service::start(Arc::new(storage)));
        let id = "a2666ff7-a305-456e-8f4e-a7733b7bc2f0";
        handle.create_entry(wiki::storage::types::KnowledgeEntryData {
            id: id.into(), title: "Native wiki images".into(), alias: vec![], tags: vec![], content: "".into(), ctime: 1, mtime: 1,
        }).await.unwrap();
        let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNQaHjwHwAExAKAc00zmAAAAABJRU5ErkJggg==").unwrap();
        let image = handle.attach_image(id.parse().unwrap(), "original.png".into(), png).await.unwrap();
        let mut entry = handle.get_entry(id.parse().unwrap()).await.unwrap();
        entry.content = format!("# Native wiki images\n\n![legacy](wiki-attach://{}/original.png)\n\n{}\n", image.id, wiki::attachments::markdown_ref(id, &image.id, "original.png"));
        handle.update_entry(entry).await.unwrap();
        let shared = bib_base::BibShared::open_in_memory().await.unwrap();
        api_server::start(api_server::api_router(shared, handle), "127.0.0.1:0").await.unwrap()
    });
    let api_addr = server.addr().to_string();
    let origin = std::env::var("WIKI_NATIVE_SMOKE_ORIGIN")
        .unwrap_or_else(|_| "http://127.0.0.1:1441".into());
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().build.dev_url = Some(origin.parse().unwrap());
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            gateway::api,
            gateway::upload_wiki_image,
            gateway::wiki_image_raw,
            smoke_done
        ])
        .setup(move |app| {
            app.manage(gateway::GatewayState::for_endpoint(
                &api_addr,
                Some("native-wiki-token".into()),
            ));
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(
                    format!("{origin}/tests/wiki-native.html").parse().unwrap(),
                ),
            )
            .title("Wiki image native qualification")
            .inner_size(1200.0, 900.0)
            .disable_drag_drop_handler()
            .build()?;
            let app = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(90));
                app.exit(2);
            });
            Ok(())
        })
        .build(context)
        .expect("build wiki native qualification");
    let code = app.run_return(|_, _| {});
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(2), server.shutdown()).await;
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    assert_eq!(code, 0, "native wiki qualification failed");
}
