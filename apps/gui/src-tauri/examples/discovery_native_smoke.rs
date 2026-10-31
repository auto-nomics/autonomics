//! Actual WebView/IPC/API/storage qualification with deterministic sources.
#[allow(dead_code)]
#[path = "../src/gateway.rs"]
mod gateway;
#[allow(dead_code)]
#[path = "../src/request_cancellation.rs"]
mod request_cancellation;
use bib_base::{
    BibShared,
    query::{LiteratureGateway, LiteratureSource, SourceBatch},
};
use bib_types::{Article, Identifier, StructuredSearch, discovery::*};
use std::{sync::Arc, time::Duration};
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

async fn arxiv_fixture() -> Arc<arxiv::ArxivClient> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/api/query", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            while !bytes.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
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
            assert!(url.query_pairs().any(|(key,value)|key=="search_query"&&value=="ti:gene ANDNOT au:Smith"),"native query changed before the actual arXiv SDK HTTP request");
            assert!(
                url.query_pairs()
                    .any(|(key, value)| key == "max_results" && value == "25")
            );
            let body = "<feed xmlns='http://www.w3.org/2005/Atom' xmlns:os='http://a9.com/-/spec/opensearch/1.1/'><os:totalResults>1</os:totalResults><os:startIndex>0</os:startIndex><os:itemsPerPage>1</os:itemsPerPage><entry><id>https://arxiv.org/abs/2401.12345v2</id><title>Native arxiv HTTP gene paper</title><summary>Gene evidence from a local Atom fixture.</summary><published>2024-01-01T00:00:00Z</published></entry></feed>";
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/atom+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    Arc::new(arxiv::ArxivClient::new().with_base_url(endpoint))
}

async fn crossref_fixture() -> Arc<crossref::CrossrefClient> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            while !bytes.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
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
            assert_eq!(url.path(), "/works");
            for (expected_key, expected_value) in [
                ("query.bibliographic", "gene editing"),
                ("filter", "type:journal-article"),
                ("rows", "25"),
            ] {
                assert!(
                    url.query_pairs()
                        .any(|(key, value)| key == expected_key && value == expected_value),
                    "native Crossref parameter changed: {expected_key}"
                );
            }
            let cursor = url
                .query_pairs()
                .find(|(key, _)| key == "cursor")
                .unwrap()
                .1
                .into_owned();
            assert!(matches!(cursor.as_str(), "*" | "native+/=cursor"));
            let first = cursor == "*";
            let items = if first {
                (0..25).map(|index| serde_json::json!({"DOI":format!("10.1000/native-crossref-{index}"), "title":[if index==0 {"Native crossref HTTP gene paper".to_owned()} else {format!("Native crossref additional paper {index}")}], "type":"journal-article"})).collect::<Vec<_>>()
            } else {
                vec![
                    serde_json::json!({"DOI":"10.1000/native-crossref-next", "title":["Native crossref next HTTP gene paper"], "type":"journal-article"}),
                ]
            };
            let body = serde_json::json!({"status":"ok", "message-type":"work-list", "message":{"total-results":26,"items":items,"next-cursor":if first {Some("native+/=cursor")} else {None}}}).to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    Arc::new(
        crossref::CrossrefClient::builder()
            .base_url(endpoint)
            .build(),
    )
}

async fn s2_fixture() -> Arc<semantic_scholar::S2Client> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    // Exactly one upstream batch; subsequent GUI pages must use its buffer.
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        while !bytes.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0);
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
        assert_eq!(url.path(), "/paper/search/bulk");
        for (key, value) in [
            ("query", "(gene | asthma) -mice"),
            ("year", "2020-2024"),
            ("sort", "paperId:asc"),
        ] {
            assert!(
                url.query_pairs()
                    .any(|(name, text)| name == key && text == value)
            );
        }
        assert!(!url.query_pairs().any(|(key, _)| key == "limit"));
        let papers=(0..30).map(|n|serde_json::json!({"paperId":format!("{:040x}",1000+n),"title":if n==0 {"Native S2 HTTP gene paper".to_owned()}else if n==29 {"Native S2 buffered last paper".to_owned()}else{format!("Native S2 extra paper {n}")},"year":2024,"externalIds":{"DOI":format!("10.1000/native-s2-{n}")},"publicationTypes":["JournalArticle"]})).collect::<Vec<_>>();
        let body = serde_json::json!({"total":30,"data":papers}).to_string();
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    });
    Arc::new(semantic_scholar::S2Client::new().with_graph_base_url(endpoint))
}

struct Source(&'static str);
#[async_trait::async_trait]
impl LiteratureSource for Source {
    fn name(&self) -> &'static str {
        self.0
    }
    fn discovery_capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            source: self.0.into(),
            searchable: true,
            native_query: true,
            fields: vec![bib_types::SearchField::Any],
            filters: Vec::new(),
            sorts: vec![DiscoverySort::Relevance],
            pagination: SourcePagination::None,
            max_page_size: 100,
            credentials_configured: false,
            authentication: SourceAuthentication::None,
            quota: None,
            notes: Vec::new(),
        }
    }
    fn plan_discovery(&self, request: &DiscoveryRequest) -> Result<SourcePlan, SourceFailure> {
        bib_base::discovery_plan::plan(self.0, request)
    }
    async fn discovery_page(
        &self,
        plan: &SourcePlan,
        _: Option<&str>,
        _: u16,
    ) -> Result<SourcePage, SourceFailure> {
        if self.0 == "openalex" {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let mut article = Article::new(
            format!("{}-native-discovery", self.0),
            if self.0 == "pubmed" {
                "Native discovery paper"
            } else {
                "Native discovery selected variant"
            },
        );
        article.year = Some(2024);
        article.abstract_text = Some("Gene evidence for the native discovery fixture.".into());
        article
            .identifiers
            .push(Identifier::doi("10.1000/native-discovery"));
        Ok(SourcePage {
            source: self.0.into(),
            articles: vec![article],
            total: Some(1),
            next_cursor: None,
            executed_parameters: plan.parameters.clone(),
            query_translation: Some("native fixture translation".into()),
            notices: Vec::new(),
        })
    }
    async fn search(&self, _: &StructuredSearch, _: usize) -> bib_base::Result<SourceBatch> {
        unreachable!()
    }
    async fn fetch(&self, _: &Identifier) -> bib_base::Result<Option<Article>> {
        unreachable!()
    }
}
#[tauri::command]
fn smoke_done(app: tauri::AppHandle, result: serde_json::Value) {
    let output = std::env::var("BIB_DISCOVERY_NATIVE_OUTPUT")
        .unwrap_or_else(|_| "/tmp/bib-discovery-native.json".into());
    std::fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("native discovery qualification: {result}");
    app.exit(if result["ok"] == true { 0 } else { 1 });
}
fn main() {
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(async {
        let mut shared = BibShared::open_in_memory()
            .await
            .unwrap()
            .with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp()));
        shared
            .bib
            .upsert_article(&Article::new("native-existing", "Existing library paper"))
            .await
            .unwrap();
        shared.arxiv = arxiv_fixture().await;
        shared.crossref = crossref_fixture().await;
        shared.s2 = s2_fixture().await;
        shared.gateway = Arc::new(
            LiteratureGateway::new()
                .with_source(Arc::new(Source("pubmed")))
                .with_source(Arc::new(Source("openalex")))
                .with_source(Arc::new(bib_base::ArxivSource::new(shared.arxiv.clone())))
                .with_source(Arc::new(bib_base::CrossrefSource::new(
                    shared.crossref.clone(),
                )))
                .with_source(Arc::new(bib_base::S2Source::new(shared.s2.clone()))),
        );
        api_server::start(
            api_server::api_router_with_auth(
                shared,
                wiki_handle().await,
                Some("native-fixture-token".into()),
            ),
            "127.0.0.1:0",
        )
        .await
        .unwrap()
    });
    let api_addr = server.addr().to_string();
    let origin = std::env::var("BIB_DISCOVERY_NATIVE_ORIGIN")
        .unwrap_or_else(|_| "http://127.0.0.1:1429".into());
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
                    format!("{origin}/tests/discovery-native.html")
                        .parse()
                        .unwrap(),
                ),
            )
            .title("Native discovery qualification")
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
        .expect("build native discovery window");
    let code = app.run_return(|_, _| {});
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(2), server.shutdown()).await;
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    assert_eq!(code, 0, "native discovery qualification failed");
}
