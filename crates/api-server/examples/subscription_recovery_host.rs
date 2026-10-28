//! Owned process-recovery fixture. Real canonical DB/API/scheduler, injected
//! source pages only. Never loads a user library or contacts an upstream API.
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use bib_base::{
    BibShared,
    query::{LiteratureGateway, LiteratureSource, SourceBatch},
};
use bib_types::{Article, Identifier, StructuredSearch, discovery::*};
use serde_json::json;
use std::{
    fs::OpenOptions,
    io::{BufRead, Write},
    path::PathBuf,
    sync::Arc,
};
type FixtureDb = Arc<tokio::sync::Mutex<turso::Connection>>;
async fn observed_lease(
    State(db): State<FixtureDb>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if headers.get("authorization").and_then(|h| h.to_str().ok())
        != Some("Bearer subscription-recovery-fixture")
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"fixture token required"})),
        ));
    }
    let read=async {
        let conn=db.lock().await;
        let mut rows=conn.query("SELECT leaseUntil,epoch,owner FROM bibliographySubscriptions WHERE id=?1",[id]).await?;
        let Some(row)=rows.next().await? else {return Ok::<_,turso::Error>(None)};
        Ok(Some(json!({"lease_until_ms":row.get::<i64>(0)?,"epoch":row.get::<i64>(1)?,"owner":row.get::<Option<String>>(2)?})))
    }.await;
    match read {
        Ok(Some(value)) => Ok(Json(value)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error":"fixture rule not found"})),
        )),
        Err(error) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":error.to_string()})),
        )),
    }
}
const TOKEN: &str = "subscription-recovery-fixture";
const TAIL: &str = "resume-after-fixture-a";
struct HoldDropped(PathBuf);
impl Drop for HoldDropped {
    fn drop(&mut self) {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.0) {
            let _ = writeln!(
                file,
                "{}",
                json!({"at_ms":chrono::Utc::now().timestamp_millis(),"pending_source_future_dropped":true})
            );
        }
    }
}
struct FixtureSource {
    root: PathBuf,
    instance: String,
    hold: bool,
}
#[async_trait::async_trait]
impl LiteratureSource for FixtureSource {
    fn name(&self) -> &'static str {
        "openalex"
    }
    fn plan_discovery(&self, request: &DiscoveryRequest) -> Result<SourcePlan, SourceFailure> {
        bib_base::discovery_plan::plan("openalex", request)
    }
    async fn search(&self, _: &StructuredSearch, _: usize) -> bib_base::Result<SourceBatch> {
        unreachable!("fixture supports discovery only")
    }
    async fn fetch(&self, _: &Identifier) -> bib_base::Result<Option<Article>> {
        unreachable!("fixture supports discovery only")
    }
    async fn discovery_page(
        &self,
        plan: &SourcePlan,
        cursor: Option<&str>,
        page_size: u16,
    ) -> Result<SourcePage, SourceFailure> {
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join(format!("{}-calls.jsonl", self.instance)))
            .unwrap();
        writeln!(log,"{}",json!({"instance":self.instance,"cursor":cursor,"at_ms":chrono::Utc::now().timestamp_millis(),"plan_fingerprint":plan.fingerprint})).unwrap();
        log.flush().unwrap();
        drop(log);
        if plan.source != "openalex" || page_size != 1 || (cursor.is_some() && cursor != Some(TAIL))
        {
            return Err(SourceFailure::new(
                SourceFailureKind::InvalidQuery,
                "unexpected recovery fixture plan/cursor",
                false,
            ));
        }
        if self.hold && cursor.is_some() {
            let _dropped = HoldDropped(self.root.join(format!("{}-dropped.jsonl", self.instance)));
            return std::future::pending().await;
        }
        let suffix = if cursor.is_none() { "a" } else { "b" };
        let mut article = Article::new(
            format!("fixture-{suffix}"),
            format!("Fixture immune paper {suffix}"),
        );
        article.identifiers.push(Identifier::doi(format!(
            "10.1000/subscription-process-{suffix}"
        )));
        Ok(SourcePage {
            source: "openalex".into(),
            articles: vec![article],
            total: Some(2),
            next_cursor: cursor.is_none().then(|| TAIL.into()),
            executed_parameters: plan.parameters.clone(),
            query_translation: None,
            notices: Vec::new(),
        })
    }
}
fn segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 5
        || !segment(&args[2])
        || !segment(&args[3])
        || !matches!(args[4].as_str(), "hold-tail" | "complete")
    {
        return Err(
            "usage: subscription_recovery_host OWNED_ROOT DATA_SEGMENT INSTANCE hold-tail|complete"
                .into(),
        );
    }
    let root = PathBuf::from(&args[1]);
    let marker: serde_json::Value = serde_json::from_slice(&std::fs::read(
        root.join("owned-subscription-fixture.json"),
    )?)?;
    if marker["schema"] != "bib-subscription-process-fixture-v1" {
        return Err("missing owned fixture marker".into());
    }
    let bootstrap = root.join(format!("{}-bootstrap.json", args[3]));
    if bootstrap.exists() {
        return Err("refusing to overwrite an existing host receipt".into());
    }
    let data = root.join(&args[2]);
    std::fs::create_dir_all(&data)?;
    let mut shared = BibShared::open(data.join("library.db")).await?;
    shared.gateway = Arc::new(
        LiteratureGateway::new().with_source(Arc::new(FixtureSource {
            root: root.clone(),
            instance: args[3].clone(),
            hold: args[4] == "hold-tail",
        })),
    );
    let service = shared.subscription_service().await?;
    let fixture_db = turso::Builder::new_local(data.join("library.db").to_string_lossy().as_ref())
        .experimental_multiprocess_wal(true)
        .build()
        .await?;
    let fixture_conn = fixture_db.connect()?;
    fixture_conn.pragma_update("busy_timeout", 5000).await?;
    let fixture_routes = axum::Router::new()
        .route("/__fixture/lease/{id}", get(observed_lease))
        .with_state(Arc::new(tokio::sync::Mutex::new(fixture_conn)));
    let server = api_server::start(
        api_server::api_router_with_auth(shared, Some(TOKEN.into())).merge(fixture_routes),
        "127.0.0.1:0",
    )
    .await?;
    std::fs::write(
        &bootstrap,
        serde_json::to_vec_pretty(
            &json!({"schema":"bib-subscription-process-host-v1","pid":std::process::id(),"origin":format!("http://{}",server.addr()),"data":data,"instance":args[3],"mode":args[4],"lease_ttl_ms":bib_base::subscription_execution::LEASE_TTL_MS,"external_sources":false}),
        )?,
    )?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = std::io::stdin().lock().read_line(&mut line);
        if result.is_ok_and(|n| n == 0 || line.trim() == "shutdown") {
            let _ = stop.send(());
        }
    });
    stopped.await?;
    service.shutdown().await;
    server.shutdown().await?;
    Ok(())
}
