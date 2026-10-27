//! Explicit native WebView qualification; run via npm run test:bib-native.
#[allow(dead_code)]
#[path = "../src/gateway.rs"]
mod gateway;
#[allow(dead_code)]
#[path = "../src/request_cancellation.rs"]
mod request_cancellation;
use std::{sync::Arc, time::Duration};
use tauri::Manager;

#[tauri::command]
fn smoke_done(app: tauri::AppHandle, result: serde_json::Value) {
    let output = std::env::var("BIB_NATIVE_SMOKE_OUTPUT")
        .unwrap_or_else(|_| "/tmp/bib-native-smoke.json".into());
    std::fs::write(&output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("native bibliography qualification: {result}");
    app.exit(if result["ok"] == true { 0 } else { 1 });
}
fn main() {
    // All environment setup precedes the runtime and WebView threads.
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(async {
        let shared = bib_base::BibShared::open_in_memory()
            .await
            .unwrap()
            .with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp()));
        let mut article = bib_types::Article::new("native-a", "Native literature evidence");
        article.year = Some(2022);
        article.pub_types = vec!["Review".into()];
        article.keywords = vec!["gene".into()];
        shared.bib.upsert_article(&article).await.unwrap();
        let db = shared.bib.clone();
        let server = api_server::start(api_server::api_router(shared), "127.0.0.1:0")
            .await
            .unwrap();
        let http = reqwest::Client::new();
        for (name, text, preferred) in [
            ("preferred.txt", "Unrelated preferred file.", true),
            (
                "matching.txt",
                "İ prefix genome.\n😀 gene editing native evidence in the second file.\n",
                false,
            ),
        ] {
            let form = reqwest::multipart::Form::new()
                .text("preferred", preferred.to_string())
                .part(
                    "file",
                    reqwest::multipart::Part::text(text)
                        .file_name(name)
                        .mime_str("text/plain")
                        .unwrap(),
                );
            let response = http
                .post(format!(
                    "http://{}/api/v1/bib/articles/native-a/attachments",
                    server.addr()
                ))
                .multipart(form)
                .send()
                .await
                .unwrap();
            assert!(
                response.status().is_success(),
                "upload: {}",
                response.text().await.unwrap()
            );
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let page = db
                    .search_v2(bib_types::SearchRequestV2 {
                        query: "fulltext:\"gene editing\"".into(),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
                if page.total == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server
    });
    let api_addr = server.addr().to_string();
    let origin =
        std::env::var("BIB_NATIVE_SMOKE_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:1428".into());
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().build.dev_url = Some(origin.parse().unwrap());
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            gateway::api,
            gateway::cancel_api_request,
            gateway::bib_fulltext_raw,
            smoke_done
        ])
        .setup(move |app| {
            app.manage(gateway::GatewayState::for_endpoint(
                &api_addr,
                Some("native-fixture-token".into()),
            ));
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(
                    format!("{origin}/tests/bibliography-native.html")
                        .parse()
                        .unwrap(),
                ),
            )
            .title("Bibliography native qualification")
            .inner_size(1440.0, 1000.0)
            .build()?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(90));
                handle.exit(2);
            });
            Ok(())
        })
        .build(context)
        .expect("build native qualification window");
    let code = app.run_return(|_, _| {});
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(2), server.shutdown()).await;
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    assert_eq!(code, 0, "native WebView qualification failed");
}
