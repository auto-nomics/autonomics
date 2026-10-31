//! Isolated P5 desktop qualification: real WebView, IPC, API, DB and scheduler.
#[allow(dead_code)]
#[path = "../src/gateway.rs"]
mod gateway;
#[allow(dead_code)]
#[path = "../src/request_cancellation.rs"]
mod request_cancellation;
use bib_base::query::{LiteratureGateway, LiteratureSource, SourceBatch};
use bib_types::{Article, Identifier, StructuredSearch, discovery::*};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tauri::Manager;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 路由状态需要真实 wiki 句柄;smoke fixture 用独立内存库。
async fn wiki_handle() -> Arc<wiki::service::WikiHandle> {
    let storage = Arc::new(
        wiki::storage::turso_wiki_storage::TursoWikiStorage::open_in_memory()
            .await
            .unwrap()
            .with_blob_store(wiki::storage::blob::MemoryBlobStore::new()),
    );
    Arc::new(wiki::service::start(storage))
}
#[derive(Default)]
struct Control {
    hold: AtomicBool,
    held: AtomicUsize,
    dropped: AtomicUsize,
    graph_calls: std::sync::Mutex<Vec<String>>,
}
struct Dropped(Arc<Control>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
struct FixtureSource {
    name: &'static str,
    control: Arc<Control>,
}
#[async_trait::async_trait]
impl LiteratureSource for FixtureSource {
    fn name(&self) -> &'static str {
        self.name
    }
    fn discovery_capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            source: self.name.into(),
            searchable: true,
            native_query: true,
            fields: vec![],
            filters: vec![],
            sorts: vec![DiscoverySort::Relevance],
            pagination: SourcePagination::Cursor,
            max_page_size: 100,
            credentials_configured: false,
            authentication: SourceAuthentication::None,
            quota: None,
            notes: vec!["Local deterministic native qualification fixture".into()],
        }
    }
    fn plan_discovery(&self, request: &DiscoveryRequest) -> Result<SourcePlan, SourceFailure> {
        bib_base::discovery_plan::plan(self.name, request)
    }
    async fn search(&self, _: &StructuredSearch, _: usize) -> bib_base::Result<SourceBatch> {
        unreachable!()
    }
    async fn fetch(&self, _: &Identifier) -> bib_base::Result<Option<Article>> {
        unreachable!()
    }
    async fn discovery_page(
        &self,
        plan: &SourcePlan,
        _: Option<&str>,
        page_size: u16,
    ) -> Result<SourcePage, SourceFailure> {
        if self.control.hold.load(Ordering::SeqCst) {
            self.control.held.fetch_add(1, Ordering::SeqCst);
            let _dropped = Dropped(self.control.clone());
            return std::future::pending().await;
        }
        let filter = plan
            .parameters
            .get("filter")
            .map(String::as_str)
            .unwrap_or("");
        let articles = [('a', "W100000001"), ('b', "W100000002")]
            .into_iter()
            .filter(|(_, id)| !filter.contains("openalex:") || filter.contains(id))
            .take(usize::from(page_size))
            .map(|(suffix, id)| {
                let mut a = Article::new(
                    format!("fixture-{suffix}"),
                    format!("Native fresh immune paper {suffix}"),
                );
                a.year = Some(2024);
                a.identifiers = vec![
                    Identifier::doi(format!("10.1000/native-p5-{suffix}")),
                    Identifier {
                        kind: bib_types::IdKind::OpenAlex,
                        value: id.into(),
                    },
                ];
                a
            })
            .collect();
        Ok(SourcePage {
            source: self.name.into(),
            articles,
            total: Some(if filter.contains("openalex:") { 1 } else { 2 }),
            next_cursor: None,
            executed_parameters: plan.parameters.clone(),
            query_translation: None,
            notices: vec![],
        })
    }
}
// The SDK talks HTTP to this owned listener; no graph adapter is mocked.
async fn graph_fixture(control: Arc<Control>) -> Arc<semantic_scholar::S2Client> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            while !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).await.unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..count]);
                assert!(bytes.len() < 65536);
            }
            let head = String::from_utf8(bytes).unwrap();
            let target = head
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let url = reqwest::Url::parse(&format!("http://fixture{target}")).unwrap();
            control.graph_calls.lock().unwrap().push(target.into());
            let a = "a".repeat(40);
            let b = "b".repeat(40);
            let paper = json!({"paperId":b,"title":"Native fresh graph paper","year":2024,"externalIds":{"DOI":"10.1000/native-p5-b"},"authors":[]});
            let response = if url.path() == format!("/paper/{a}/references") {
                assert!(url.query_pairs().any(|(k, v)| k == "limit" && v == "25"));
                json!({"offset":0,"data":[{"citedPaper":paper,"contexts":["Native citation context"],"intents":["background"],"isInfluential":true},{"citedPaper":{"paperId":null,"title":"Native unresolved reference"}}]})
            } else if url.path() == format!("/paper/{a}/citations") {
                json!({"offset":0,"data":[{"citingPaper":paper}]})
            } else if url.path() == format!("/papers/forpaper/{a}") {
                assert!(
                    url.query_pairs()
                        .any(|(k, v)| k == "from" && (v == "recent" || v == "all-cs"))
                );
                json!({"recommendedPapers":[paper]})
            } else if url.path() == format!("/paper/{b}") {
                paper
            } else {
                json!({"error":"unexpected native graph fixture route"})
            };
            let body = response.to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    Arc::new(
        semantic_scholar::S2Client::new()
            .with_graph_base_url(&origin)
            .with_reco_base_url(&origin)
            .with_max_retries(0),
    )
}
struct Fixture {
    root: PathBuf,
    control: Arc<Control>,
}
#[tauri::command]
fn smoke_hold_source(state: tauri::State<'_, Fixture>, hold: bool) -> Value {
    state.control.hold.store(hold, Ordering::SeqCst);
    json!({"held":state.control.held.load(Ordering::SeqCst),"dropped":state.control.dropped.load(Ordering::SeqCst)})
}
#[tauri::command]
fn smoke_info(state: tauri::State<'_, Fixture>) -> Value {
    json!({"held":state.control.held.load(Ordering::SeqCst),"dropped":state.control.dropped.load(Ordering::SeqCst),"export_path":state.root.join("native-snapshot.json"),"graph_calls":state.control.graph_calls.lock().unwrap().clone()})
}
#[tauri::command]
fn smoke_done(app: tauri::AppHandle, state: tauri::State<'_, Fixture>, mut result: Value) {
    if result["ok"] == true {
        let exported = std::fs::read_to_string(state.root.join("native-snapshot.json"));
        if !exported.is_ok_and(|s| {
            s.contains("User curated native title") && !s.contains("Changed after snapshot")
        }) {
            result["ok"] = json!(false);
            result["error"] = json!("Native export missing or not frozen");
        }
    }
    result["graph_requests"] = json!(state.control.graph_calls.lock().unwrap().clone());
    result["source_future_held"] = json!(state.control.held.load(Ordering::SeqCst));
    result["source_future_dropped"] = json!(state.control.dropped.load(Ordering::SeqCst));
    std::fs::write(
        state.root.join("receipt.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    println!("P5 native result: {result}");
    app.exit(if result["ok"] == true { 0 } else { 1 });
}
fn main() {
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let root =
        PathBuf::from(std::env::var("BIB_P5_NATIVE_ROOT").expect("owned fixture root required"));
    assert_eq!(
        std::fs::read_to_string(root.join("owned-marker")).unwrap(),
        "bib-p5-native-v1"
    );
    assert!(!root.join("receipt.json").exists(), "fresh root required");
    let origin = std::env::var("BIB_NATIVE_SMOKE_ORIGIN").unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let control = Arc::new(Control::default());
    let (server, service) = runtime.block_on(async {
        let mut shared = bib_base::BibShared::open(root.join("library.db"))
            .await
            .unwrap();
        shared.s2 = graph_fixture(control.clone()).await;
        let mut article = Article::new("native-curated", "User curated native title");
        article.identifiers = vec![
            Identifier::doi("10.1000/native-p5-a"),
            Identifier::new(bib_types::IdKind::S2, "a".repeat(40)),
        ];
        article.keywords = vec!["manual-keyword".into()];
        article.year = Some(2024);
        shared.bib.upsert_article(&article).await.unwrap();
        shared.gateway = Arc::new(
            LiteratureGateway::new()
                .with_source(Arc::new(bib_base::query::S2Source::new(shared.s2.clone())))
                .with_source(Arc::new(FixtureSource {
                    name: "openalex",
                    control: control.clone(),
                }))
                .with_source(Arc::new(FixtureSource {
                    name: "pubmed",
                    control: control.clone(),
                })),
        );
        let service = shared.subscription_service().await.unwrap();
        let server = api_server::start(
            api_server::api_router_with_auth(
                shared,
                wiki_handle().await,
                Some("native-p5-fixture".into()),
            ),
            "127.0.0.1:0",
        )
        .await
        .unwrap();
        (server, service)
    });
    let api_addr = server.addr().to_string();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().build.dev_url = Some(origin.parse().unwrap());
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            gateway::api,
            gateway::cancel_api_request,
            gateway::bib_fulltext_raw,
            gateway::save_bib_export,
            smoke_hold_source,
            smoke_info,
            smoke_done
        ])
        .setup(move |app| {
            app.manage(gateway::GatewayState::for_endpoint(
                &api_addr,
                Some("native-p5-fixture".into()),
            ));
            app.manage(Fixture {
                root: root.clone(),
                control,
            });
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(
                    format!("{origin}/tests/literature-p5-native.html")
                        .parse()
                        .unwrap(),
                ),
            )
            .title("P5 native qualification")
            .inner_size(1440., 1000.)
            .data_directory(root.join("webview"))
            .build()?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(180));
                handle.exit(2);
            });
            Ok(())
        })
        .build(context)
        .expect("native P5 window");
    let code = app.run_return(|_, _| {});
    runtime.block_on(async {
        service.shutdown().await;
        let _ = server.shutdown().await;
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    assert_eq!(code, 0, "native P5 qualification failed");
}
