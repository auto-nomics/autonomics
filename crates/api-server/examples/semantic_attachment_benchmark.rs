//! End-to-end synthetic TXT fanout, HTTP queries, and sustained canonical writes.
//! semantic_attachment_benchmark FRESH_DIRECTORY COUNT FIXTURE_ORIGIN
//! Run with benchmark_bib_attachments.py; vectors are synthetic, not IR gold.
use bib_base::{
    BibShared,
    semantic_index::SemanticProfile,
    semantic_limits::SemanticLimits,
    semantic_service::{SemanticConfig, SemanticPhase, SemanticStatus},
};
use bib_types::{Article, ExtractStatus, SearchMode, SearchPageV2, SearchRequestV2};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;
const TOKEN: &str = "attachment-capacity-fixture";
fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}
fn checkpoint(root: &Path, phase: &str, details: Value) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("stages.jsonl"))?;
    writeln!(
        file,
        "{}",
        json!({"phase":phase,"at":chrono::Utc::now(),"details":details})
    )?;
    file.sync_all()?;
    eprintln!("stage: {phase}");
    Ok(())
}
fn distribution(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    json!({"samples":values.len(),"p50_ms":values[values.len()/2],"p95_ms":values[(values.len()*95).div_ceil(100)-1],"max_ms":values.last()})
}
fn article(i: usize) -> Article {
    let mut a = Article::new(
        format!("fanout-{i:09}"),
        format!("Immune study {i} cohortg{:03}", i % 100),
    );
    a.abstract_text = Some(format!(
        "Study {i} examines immune gene expression in human cells."
    ));
    a.year = Some(2000 + (i % 27) as u16);
    a
}
fn body(i: usize, slot: usize) -> String {
    // Every 512-character window has its own identity: no boilerplate dedup win.
    (0..3).map(|window| {
        let mut text = format!("attachmentneedle{i}slot{slot} Study {i} attachment {slot} window {window}. Immune gene expression. ");
        text.push_str(&"x".repeat(512-text.len()));
        text
    }).collect()
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
    reused: Arc<AtomicUsize>,
    restarted: Arc<AtomicUsize>,
    recreated: Arc<AtomicUsize>,
}
impl Api {
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
    async fn ready(&self, stage: &str, chunks: usize) -> Result<SemanticStatus> {
        let revision = self
            .shared
            .bib
            .search_index_status()
            .await
            .map_err(|error| format!("{stage}: source status: {error}"))?
            .source_revision;
        tokio::time::timeout(Duration::from_secs(3600), async {
            loop {
                let s = self
                    .status()
                    .await
                    .map_err(|error| format!("{stage}: semantic status HTTP: {error}"))?;
                if s.phase == SemanticPhase::Failed
                    && !s
                        .last_error
                        .as_deref()
                        .unwrap_or("")
                        .starts_with("search conflict:")
                {
                    return Err::<_, Error>(
                        format!(
                            "{stage}: semantic worker failed: {}",
                            serde_json::to_string(&s)?
                        )
                        .into(),
                    );
                }
                if s.phase == SemanticPhase::Ready && s.indexed_revision == Some(revision) {
                    assert_eq!(s.indexed_chunks, chunks);
                    return Ok(s);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|error| format!("{stage}: semantic convergence timeout: {error}"))?
    }
    async fn upload(&self, i: usize, slot: usize) -> Result<String> {
        let text = body(i, slot);
        let boundary = "bib-owned-fixture-boundary";
        let multipart = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"preferred\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"study-{i}-{slot}.txt\"\r\nContent-Type: text/plain\r\n\r\n{text}\r\n--{boundary}--\r\n",
            slot == 0
        );
        let response = self
            .shared
            .http
            .post(format!(
                "{}/api/v1/bib/articles/fanout-{i:09}/attachments",
                self.base
            ))
            .bearer_auth(TOKEN)
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(multipart)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            // These are public synthetic fixture requests. Retain the bounded
            // API error body; error_for_status otherwise discards the cause.
            let bytes = response.bytes().await?;
            let text = String::from_utf8_lossy(&bytes[..bytes.len().min(16384)]);
            return Err(
                format!("upload failed: article={i} slot={slot} HTTP {status}: {text}").into(),
            );
        }
        let response: Value = response.json().await?;
        let key = response["attachment"]["key"]
            .as_str()
            .ok_or("missing attachment key")?
            .to_owned();
        self.extracted(i, slot, &key).await?;
        Ok(key)
    }
    async fn extracted(&self, i: usize, slot: usize, key: &str) -> Result<()> {
        let text = body(i, slot);
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                if let Some(ft) = self
                    .shared
                    .bib
                    .get_attachment_fulltext(&format!("fanout-{i:09}"), key)
                    .await?
                {
                    match ft.extract_status {
                        Some(ExtractStatus::Done) => {
                            assert_eq!(ft.text_content.as_deref(), Some(text.as_str()));
                            return Ok::<_, Error>(());
                        }
                        Some(ExtractStatus::Failed) => {
                            return Err(ft.extract_error.unwrap_or_default().into());
                        }
                        _ => {}
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        Ok(())
    }
    async fn ensure(
        &self,
        i: usize,
        slot: usize,
        existing: &[bib_base::StoredAttachment],
    ) -> Result<String> {
        let filename = format!("study-{i}-{slot}.txt");
        let found = existing
            .iter()
            .filter(|a| a.original_filename.as_deref() == Some(filename.as_str()))
            .collect::<Vec<_>>();
        if found.len() > 1 {
            return Err(format!("duplicate fixture attachment {filename}").into());
        }
        let Some(attachment) = found.first() else {
            return self.upload(i, slot).await;
        };
        if attachment.vfs_path.is_none() {
            // A stopped HTTP upload may have inserted its child row before
            // storing bytes/metadata. This directory is an explicit copied,
            // incomplete synthetic fixture: delete and re-upload through the
            // real API, recording the recovery rather than faking completion.
            eprintln!(
                "recover incomplete upload: article={i} slot={slot} key={}",
                attachment.key
            );
            let response = self
                .shared
                .http
                .delete(format!(
                    "{}/api/v1/bib/articles/fanout-{i:09}/attachments/{}",
                    self.base, attachment.key
                ))
                .bearer_auth(TOKEN)
                .send()
                .await?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!(
                    "partial upload cleanup HTTP {status}: {}",
                    response.text().await?
                )
                .into());
            }
            let raw = bib_base::stored_files::stored_attachment(
                &format!("fanout-{i:09}"),
                &attachment.key,
                &filename,
                body(i, slot).as_bytes(),
            );
            if let Some(path) = bib_base::stored_files::vfs_virtual_path(&raw.path) {
                self.shared
                    .file_storage
                    .as_ref()
                    .ok_or("fixture VFS missing")?
                    .delete_object(&path)
                    .await?;
            }
            self.recreated.fetch_add(1, Ordering::Relaxed);
            return self.upload(i, slot).await;
        }
        let response = self
            .shared
            .http
            .get(format!(
                "{}/api/v1/bib/articles/fanout-{i:09}/attachments/{}/raw",
                self.base, attachment.key
            ))
            .bearer_auth(TOKEN)
            .send()
            .await?
            .error_for_status()?;
        assert_eq!(response.text().await?, body(i, slot));
        let ft = self
            .shared
            .bib
            .get_attachment_fulltext(&format!("fanout-{i:09}"), &attachment.key)
            .await?;
        if !ft.is_some_and(|f| f.extract_status == Some(ExtractStatus::Done)) {
            self.shared
                .http
                .post(format!(
                    "{}/api/v1/bib/articles/fanout-{i:09}/attachments/{}/fulltext/reextract",
                    self.base, attachment.key
                ))
                .bearer_auth(TOKEN)
                .send()
                .await?
                .error_for_status()?;
            self.restarted.fetch_add(1, Ordering::Relaxed);
        }
        self.extracted(i, slot, &attachment.key).await?;
        self.reused.fetch_add(1, Ordering::Relaxed);
        Ok(attachment.key.clone())
    }
    async fn page(&self, req: &SearchRequestV2) -> Result<SearchPageV2> {
        Ok(self
            .shared
            .http
            .post(format!("{}/api/v1/bib/search/v2", self.base))
            .bearer_auth(TOKEN)
            .json(req)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
    async fn query(&self, req: &SearchRequestV2, eligible: usize) -> Result<f64> {
        let start = Instant::now();
        let page = self.page(req).await?;
        assert_eq!(page.retrieval.as_ref().unwrap().eligible_articles, eligible);
        assert!(!page.hits.is_empty());
        assert_eq!(page.hits.len(), page.articles.len());
        assert!(page.facets.is_some());
        Ok(elapsed(start))
    }
    async fn lexical(&self, query: String) -> Result<SearchPageV2> {
        self.page(&SearchRequestV2 {
            query,
            include_articles: true,
            ..Default::default()
        })
        .await
    }
}
fn memory() -> Value {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let read = |key: &str| {
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    json!({"rss_kib":read("VmRSS:"),"peak_rss_kib":read("VmHWM:")})
}
fn disk(path: &Path) -> Result<(u64, u64)> {
    let (mut bytes, mut files) = (0, 0);
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let m = entry.metadata()?;
        if m.is_dir() {
            let (b, f) = disk(&entry.path())?;
            bytes += b;
            files += f;
        } else {
            bytes += m.len();
            files += 1;
        }
    }
    Ok((bytes, files))
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let resume = args.len() == 4 && args[3] == "--resume-existing";
    if args.len() != 3 && !resume {
        return Err(
            "usage: semantic_attachment_benchmark DIRECTORY COUNT FIXTURE_ORIGIN [--resume-existing]".into(),
        );
    }
    let count: usize = args[1].parse()?;
    if count < 100 || count % 100 != 0 {
        return Err("count must be a positive multiple of 100".into());
    }
    let root = Path::new(&args[0]);
    if resume {
        if !root.join("library.db").is_file() || root.join("receipt.json").exists() {
            return Err(
                "resume requires an incomplete fixture with a database and no final receipt".into(),
            );
        }
    } else {
        std::fs::create_dir(root)?;
    }
    let storage = Arc::new(vfs::OpendalFileStorage::new(root.join("vfs")));
    let shared = BibShared::open(root.join("library.db"))
        .await?
        .with_file_storage(storage);
    let server = api_server::start(
        api_server::api_router_with_auth(shared.clone(), None, Some(TOKEN.into())),
        "127.0.0.1:0",
    )
    .await?;
    let api = Api {
        shared,
        base: format!("http://{}", server.addr()),
        reused: Arc::new(AtomicUsize::new(0)),
        restarted: Arc::new(AtomicUsize::new(0)),
        recreated: Arc::new(AtomicUsize::new(0)),
    };
    let start = Instant::now();
    if resume {
        assert!(
            !api.status().await?.config.enabled,
            "cannot resume a fixture that already entered indexing or query mutation"
        );
        assert_eq!(api.lexical(String::new()).await?.total, count);
    } else {
        for begin in (0..count).step_by(500) {
            api.shared
                .bib
                .upsert_articles(
                    &(begin..(begin + 500).min(count))
                        .map(article)
                        .collect::<Vec<_>>(),
                )
                .await?;
        }
    }
    let import_ms = if resume { None } else { Some(elapsed(start)) };
    std::fs::write(
        root.join("progress.json"),
        serde_json::to_vec_pretty(
            &json!({"schema":"bib-attachment-progress-v1","phase":"upload_extract","count":count,"resumed":resume,"import_ms":import_ms}),
        )?,
    )?;
    let start = Instant::now();
    let mut first_keys = Vec::new();
    // Bounded upload workers; each waits for extraction before another batch.
    for begin in (0..count).step_by(16) {
        let mut tasks = tokio::task::JoinSet::new();
        for i in begin..(begin + 16).min(count) {
            let api = api.clone();
            tasks.spawn(async move {
                let existing = if resume {
                    api.shared
                        .bib
                        .zotero_attachments(&format!("fanout-{i:09}"))
                        .await?
                } else {
                    Vec::new()
                };
                if existing.len() > 2
                    || existing.iter().any(|a| {
                        ![format!("study-{i}-0.txt"), format!("study-{i}-1.txt")]
                            .iter()
                            .any(|n| a.original_filename.as_ref() == Some(n))
                    })
                {
                    return Err("unexpected attachment in owned resume fixture".into());
                }
                let a = api.ensure(i, 0, &existing).await?;
                let b = api.ensure(i, 1, &existing).await?;
                Ok::<_, Error>((i, a, b))
            });
        }
        while let Some(result) = tasks.join_next().await {
            let (i, a, b) = result??;
            if i == 0 {
                first_keys = vec![a, b];
            }
        }
        if begin % 1000 < 16 {
            eprintln!(
                "uploaded and extracted {}/{} articles ({:.1}s)",
                (begin + 16).min(count),
                count,
                start.elapsed().as_secs_f64()
            );
        }
    }
    let upload_extract_ms = elapsed(start);
    std::fs::write(
        root.join("progress.json"),
        serde_json::to_vec_pretty(
            &json!({"schema":"bib-attachment-progress-v1","phase":"semantic_build","count":count,"resumed":resume,"import_ms":import_ms,"upload_extract_ms":upload_extract_ms,"reused_attachments":api.reused.load(Ordering::Relaxed),"restarted_extractions":api.restarted.load(Ordering::Relaxed),"recreated_partial_uploads":api.recreated.load(Ordering::Relaxed)}),
        )?,
    )?;
    let start = Instant::now();
    let config = SemanticConfig {
        enabled: true,
        origin: args[2].clone(),
        model: "fixture:latest".into(),
        model_digest: "a".repeat(64),
        profile: SemanticProfile {
            chunk_chars: Some(512),
            include_fulltext: true,
            ..Default::default()
        },
        limits: SemanticLimits {
            max_vector_bytes: 4 * 1024 * 1024 * 1024,
            max_disk_bytes: 16 * 1024 * 1024 * 1024,
            ..Default::default()
        },
        ..Default::default()
    };
    api.shared
        .http
        .put(format!("{}/api/v1/bib/search/semantic", api.base))
        .bearer_auth(TOKEN)
        .json(&config)
        .send()
        .await?
        .error_for_status()?;
    checkpoint(
        root,
        "initial_index_wait",
        json!({"expected_chunks":count*7}),
    )?;
    let initial = api.ready("initial_index", count * 7).await?;
    let initial_ready_ms = elapsed(start);
    checkpoint(
        root,
        "initial_index_ready",
        json!({"elapsed_ms":initial_ready_ms,"status":initial}),
    )?;
    eprintln!(
        "{} unique 1024D blocks ready in {:.1}s",
        count * 7,
        initial_ready_ms / 1000.
    );
    let mut single = BTreeMap::new();
    for (name, narrow) in [("wide", false), ("one_percent", true)] {
        let expected = if narrow { count / 100 } else { count };
        let req = request(narrow);
        let first = api.query(&req, expected).await?;
        let mut samples = Vec::new();
        for _ in 0..30 {
            samples.push(api.query(&req, expected).await?);
        }
        single.insert(
            name,
            json!({"first_observed_ms":first,"hot":distribution(samples)}),
        );
    }
    let mut tasks = tokio::task::JoinSet::new();
    let start = Instant::now();
    for _ in 0..4 {
        let api = api.clone();
        tasks.spawn(async move {
            let mut samples = Vec::new();
            for _ in 0..5 {
                samples.push(api.query(&request(false), count).await?);
            }
            Ok::<_, Error>(samples)
        });
    }
    let mut concurrent = Vec::new();
    while let Some(result) = tasks.join_next().await {
        concurrent.extend(result??);
    }
    let four_clients = json!({"latency":distribution(concurrent),"wall_ms":elapsed(start)});
    checkpoint(
        root,
        "query_measurements_complete",
        json!({"single_client":single,"four_clients":four_clients,"memory":memory()}),
    )?;
    // Confirm the nonpreferred source is a real stored file and searchable body.
    let needle = "attachmentneedle0slot1";
    let lexical = api.lexical(format!("fulltext:{needle}")).await?;
    assert_eq!(lexical.total, 1);
    assert_eq!(lexical.hits[0].article_id, "fanout-000000000");
    let evidence:Value=api.shared.http.post(format!("{}/api/v1/bib/articles/fanout-000000000/search-evidence",api.base)).bearer_auth(TOKEN)
        .json(&json!({"search":{"query":format!("fulltext:{needle}")},"index_generation":lexical.index_generation})).send().await?.error_for_status()?.json().await?;
    assert!(
        evidence["evidence"]
            .as_array()
            .ok_or("no evidence")?
            .iter()
            .any(|e| e["attachment_key"] == first_keys[1]
                && e["snippet"].as_str().is_some_and(|s| s.contains(needle)))
    );
    let raw = api
        .shared
        .http
        .get(format!(
            "{}/api/v1/bib/articles/fanout-000000000/attachments/{}/raw",
            api.base, first_keys[1]
        ))
        .bearer_auth(TOKEN)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(raw, body(0, 1));
    let old_page = api.page(&request(false)).await?;
    checkpoint(
        root,
        "metadata_write_burst",
        json!({"iterations":40,"interval_ms":100}),
    )?;
    let start = Instant::now();
    let writer_api = api.clone();
    let writer = tokio::spawn(async move {
        let mut latencies = Vec::new();
        for iteration in 0..40 {
            let start = Instant::now();
            let mut a = article(1);
            a.title.push_str(&format!(" writeepoch{iteration}"));
            writer_api.shared.bib.upsert_articles(&[a]).await?;
            latencies.push(elapsed(start));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok::<_, Error>(distribution(latencies))
    });
    let mut responses = BTreeMap::<u16, usize>::new();
    let mut lexical_samples = Vec::new();
    while !writer.is_finished() {
        let started = Instant::now();
        assert!(api.lexical("title:Immune".into()).await?.total > 0);
        lexical_samples.push(elapsed(started));
        let response = api
            .shared
            .http
            .post(format!("{}/api/v1/bib/search/v2", api.base))
            .bearer_auth(TOKEN)
            .json(&request(false))
            .send()
            .await?;
        let status = response.status().as_u16();
        assert!(
            [200, 409, 503].contains(&status),
            "unexpected status {status}"
        );
        if status == 200 {
            let page = response.json::<SearchPageV2>().await?;
            assert_eq!(page.retrieval.as_ref().unwrap().eligible_articles, count);
        }
        *responses.entry(status).or_default() += 1;
    }
    let writes = writer
        .await?
        .map_err(|error| format!("metadata write burst: {error}"))?;
    checkpoint(
        root,
        "metadata_convergence_wait",
        json!({"write_latency":writes,"lexical_samples":lexical_samples,"hybrid_http_status_counts":responses}),
    )?;
    let metadata_final = api.ready("metadata_convergence", count * 7).await?;
    assert_eq!(api.lexical("title:writeepoch39".into()).await?.total, 1);
    let writer_wall_ms = elapsed(start);
    checkpoint(
        root,
        "metadata_converged",
        json!({"wall_ms":writer_wall_ms,"status":metadata_final}),
    )?;
    // Production delete/upload changes both canonical identity and real VFS bytes.
    api.shared
        .http
        .delete(format!(
            "{}/api/v1/bib/articles/fanout-000000000/attachments/{}",
            api.base, first_keys[1]
        ))
        .bearer_auth(TOKEN)
        .send()
        .await?
        .error_for_status()?;
    api.ready("attachment_delete", count * 7 - 3).await?;
    assert_eq!(api.lexical(format!("fulltext:{needle}")).await?.total, 0);
    let replacement_key = api.upload(0, 2).await?;
    assert_ne!(replacement_key, first_keys[1]);
    let final_status = api.ready("attachment_replacement", count * 7).await?;
    assert_eq!(
        api.lexical("fulltext:attachmentneedle0slot2".into())
            .await?
            .total,
        1
    );
    if let Some(cursor) = old_page.next_cursor {
        let mut req = request(false);
        req.cursor = Some(cursor);
        let response = api
            .shared
            .http
            .post(format!("{}/api/v1/bib/search/v2", api.base))
            .bearer_auth(TOKEN)
            .json(&req)
            .send()
            .await?;
        assert_eq!(response.status().as_u16(), 400);
        let rejected: Value = response.json().await?;
        assert!(
            rejected["error"]
                .as_str()
                .is_some_and(|message| message.contains("cursor expired"))
        );
    } else {
        return Err("fixture must produce a pagination cursor".into());
    }
    let (bytes, files) = disk(&root.join("vfs"))?;
    let receipt = json!({"schema":"bib-semantic-attachment-capacity-v1","count":count,"controlled_http_model_fixture":true,
        "attachments_per_article":2,"chars_per_attachment":1536,"expected_chunks":count*7,"dimensions":1024,"config":config,
        "resumed":resume,"reused_attachments":api.reused.load(Ordering::Relaxed),"restarted_extractions":api.restarted.load(Ordering::Relaxed),"recreated_partial_uploads":api.recreated.load(Ordering::Relaxed),
        "import_ms":import_ms,"upload_extract_ms":upload_extract_ms,"initial_ready_ms":initial_ready_ms,
        "single_client":single,"four_clients":four_clients,"memory":memory(),"initial_status":initial,
        "sustained_writes":{"iterations":40,"interval_ms":100,"wall_ms":writer_wall_ms,"write_latency":writes,"lexical":distribution(lexical_samples),"hybrid_http_status_counts":responses,"final_status":metadata_final},
        "nonpreferred_raw_and_evidence_verified":true,"delete_and_new_attachment_verified":true,"old_cursor_rejected":true,
        "vfs":{"logical_file_bytes":bytes,"files":files},"final_status":final_status});
    std::fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{}", serde_json::to_string(&receipt)?);
    server.shutdown().await?;
    Ok(())
}
