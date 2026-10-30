//! Real TCP/HTTP + auth + JSON benchmark using a copied synthetic library.
//! Run with scripts/benchmark_bib_semantic_service.py --reuse-from ... .
use bib_base::{
    BibShared,
    semantic_service::{SemanticConfig, SemanticPhase, SemanticStatus},
};
use bib_types::{SearchMode, SearchPageV2, SearchRequestV2};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;
const TOKEN: &str = "capacity-fixture-token";
fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}
fn distribution(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    json!({"samples":samples.len(),"p50_ms":samples[samples.len()/2],"p95_ms":samples[(samples.len()*95).div_ceil(100)-1],"max_ms":samples.last()})
}
fn request(narrow: bool) -> SearchRequestV2 {
    SearchRequestV2 {
        mode: SearchMode::Hybrid,
        semantic_query: Some("immune gene expression".into()),
        query: if narrow {
            "title:cohortg001".into()
        } else {
            String::new()
        },
        include_facets: true,
        include_articles: true,
        page_size: 20,
        ..Default::default()
    }
}
#[derive(Clone)]
struct Api {
    shared: BibShared,
    base: String,
}
impl Api {
    async fn page(&self, request: &SearchRequestV2, eligible: usize) -> Result<SearchPageV2> {
        let response = self
            .shared
            .http
            .post(format!("{}/api/v1/bib/search/v2", self.base))
            .bearer_auth(TOKEN)
            .json(request)
            .send()
            .await?
            .error_for_status()?;
        let page = response.json::<SearchPageV2>().await?;
        assert_eq!(page.retrieval.as_ref().unwrap().eligible_articles, eligible);
        assert!(!page.hits.is_empty());
        assert!(page.total <= 200);
        assert!(page.facets.is_some());
        assert_eq!(page.hits.len(), page.articles.len());
        Ok(page)
    }
    async fn query(&self, request: &SearchRequestV2, eligible: usize) -> Result<f64> {
        let start = Instant::now();
        self.page(request, eligible).await?;
        Ok(elapsed(start))
    }
    async fn status(&self) -> Result<SemanticStatus> {
        Ok(self
            .shared
            .http
            .get(format!("{}/api/v1/bib/search/semantic", self.base))
            .bearer_auth(TOKEN)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}
fn memory() -> Value {
    let text = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let read = |key: &str| {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    json!({"rss_kib":read("VmRSS:"),"peak_rss_kib":read("VmHWM:")})
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 || args[3] != "--reuse-existing" {
        return Err(
            "usage: semantic_http_benchmark COPIED_DIRECTORY COUNT FIXTURE_ORIGIN --reuse-existing"
                .into(),
        );
    }
    let root = Path::new(&args[0]);
    if !root.join("library.db").is_file() {
        return Err("copied fixture database required".into());
    }
    let count: usize = args[1].parse()?;
    if count < 100 || count % 100 != 0 {
        return Err("count must be a positive multiple of 100".into());
    }
    let started = Instant::now();
    let shared = BibShared::open(root.join("library.db")).await?;
    let open_ms = elapsed(started);
    let start = Instant::now();
    let server = api_server::start(
        api_server::api_router_with_auth(shared.clone(), None, Some(TOKEN.into())),
        "127.0.0.1:0",
    )
    .await?;
    let listen_ms = elapsed(start);
    let api = Api {
        shared,
        base: format!("http://{}", server.addr()),
    };
    assert_eq!(
        api.shared
            .http
            .get(format!("{}/api/v1/bib/search/semantic", api.base))
            .send()
            .await?
            .status()
            .as_u16(),
        401
    );
    let start = Instant::now();
    let configured = api
        .shared
        .http
        .put(format!("{}/api/v1/bib/search/semantic", api.base))
        .bearer_auth(TOKEN)
        .json(&SemanticConfig {
            enabled: true,
            origin: args[2].clone(),
            model: "fixture:latest".into(),
            model_digest: "a".repeat(64),
            ..Default::default()
        })
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let configuration_generation = configured["configuration_generation"]
        .as_str()
        .ok_or("missing configuration epoch")?
        .to_owned();
    let initial = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let status = api.status().await?;
            if status.configuration_generation != configuration_generation {
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
            if status.phase == SemanticPhase::Failed {
                return Err::<_, Error>(status.last_error.unwrap_or_default().into());
            }
            if status.phase == SemanticPhase::Ready {
                assert_eq!(status.indexed_chunks, count);
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let initial_ready_ms = elapsed(start);
    eprintln!("{count} vectors loaded through HTTP status in {initial_ready_ms:.0}ms");
    if let Ok(driver) = std::env::var("BIB_GUI_CAPACITY_DRIVER") {
        let gui_path = root.join("gui-receipt.json");
        let gui_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .ok_or("missing workspace root")?
            .join("apps/gui");
        let status = tokio::process::Command::new("node")
            .arg(driver)
            .arg(&api.base)
            .arg(TOKEN)
            .arg(count.to_string())
            .arg(&gui_path)
            .current_dir(gui_root)
            .status()
            .await?;
        if !status.success() {
            return Err("browser capacity driver failed".into());
        }
        let gui: Value = serde_json::from_slice(&std::fs::read(&gui_path)?)?;
        server.shutdown().await?;
        let receipt = json!({"schema":"bib-semantic-browser-capacity-v1","count":count,"dimensions":1024,
            "controlled_http_model_fixture":true,"copied_fixture_reused":true,"relevance_evaluation":false,
            "open_ms":open_ms,"initial_ready_ms":initial_ready_ms,"gui":gui,
            "single_client":{"wide":{"hot":gui["enter_to_visible"]}},"four_clients":null});
        std::fs::write(
            root.join("receipt.json"),
            serde_json::to_vec_pretty(&receipt)?,
        )?;
        println!("{receipt}");
        return Ok(());
    }
    let mut single = BTreeMap::new();
    let mut first_result_total_ms = None;
    for (name, narrow) in [("wide", false), ("one_percent", true)] {
        let eligible = if narrow { count / 100 } else { count };
        let first = api.query(&request(narrow), eligible).await?;
        if !narrow {
            first_result_total_ms = Some(elapsed(started));
        }
        let mut samples = Vec::new();
        for _ in 0..30 {
            samples.push(api.query(&request(narrow), eligible).await?);
        }
        let hot = distribution(samples);
        eprintln!("{count} HTTP {name} hot P95 {}ms", hot["p95_ms"]);
        single.insert(name, json!({"first_observed_ms":first,"hot":hot}));
    }
    let barrier = Arc::new(tokio::sync::Barrier::new(4));
    let mut tasks = Vec::new();
    let start = Instant::now();
    for _ in 0..4 {
        let api = api.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut samples = Vec::new();
            for _ in 0..5 {
                samples.push(api.query(&request(false), count).await?);
            }
            Ok::<_, Error>(samples)
        }));
    }
    let mut concurrent = Vec::new();
    for task in tasks {
        concurrent.extend(task.await??);
    }
    let concurrent_wall_ms = elapsed(start);
    let first = api.page(&request(false), count).await?;
    let mut next = request(false);
    next.cursor = Some(first.next_cursor.ok_or("missing continuation")?);
    let second = api.page(&next, count).await?;
    assert!(
        first
            .hits
            .iter()
            .all(|a| second.hits.iter().all(|b| a.article_id != b.article_id))
    );
    let final_status = api.status().await?;
    assert_eq!(
        final_status.configuration_generation,
        initial.configuration_generation
    );
    server.shutdown().await?;
    let receipt = json!({"schema":"bib-semantic-http-capacity-v1","count":count,"dimensions":1024,
        "build_profile":if cfg!(debug_assertions){"debug"}else{"release"},"controlled_http_model_fixture":true,
        "copied_fixture_reused":true,"relevance_evaluation":false,"transport":"real loopback TCP/HTTP, bearer auth, JSON request and response decode",
        "open_ms":open_ms,"listen_ms":listen_ms,"initial_ready_ms":initial_ready_ms,"first_result_total_ms":first_result_total_ms,
        "single_client":single,"four_clients":distribution(concurrent),"four_clients_wall_ms":concurrent_wall_ms,
        "unauthenticated_status":401,"continuation_disjoint":true,"final_memory":memory(),
        "limitations":["Synthetic 1024D model HTTP fixture, not real model inference speed or relevance","Reopened copied library, not initial import or embedding generation","File-cache advice, when used by runner, is advisory and excludes physical storage caches","No GUI paint, input, IPC or manual accessibility timing"]});
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{receipt}");
    Ok(())
}
