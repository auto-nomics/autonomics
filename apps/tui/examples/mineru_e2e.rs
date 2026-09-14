//! Manual end-to-end check of the MinerU figure pipeline (plan phases A–E).
//!
//! ```text
//! cargo run -p tui --example mineru_e2e --release
//! ```
//!
//! Requires `MINERU_API_KEY` (loaded from the repo-root `.env`) and
//! `ANTHROPIC_AUTH_TOKEN` (the Zhipu LLM key) in the environment. The
//! vision model is `glm-5.3-flash` (vision + function calling — the
//! smaller vision flashes ignore tools) and the text control `glm-5-turbo`.
//!
//! Verified steps:
//! 1. upload a multi-figure PDF (arXiv 1706.03762) → MinerU cloud extraction
//! 2. figures land in VFS, markdown references them (`images/<hash>.jpg`)
//! 3. `GET /articles/{id}/fulltext/images/{name}` → 200 + real image bytes
//! 4. vision agent (glm-5.3-flash) calls `bib_read_figure` and describes the
//!    actual image (the tool-result image companion path)
//! 5. replace upload with a second PDF (arXiv 1810.04805) → old figure
//!    objects deleted, new ones served
//! 6. text-only control (glm-5-turbo) answers from the summary alone, no error

use agentik_core::Agent;
use agentik_core::agent::InternalEvent;
use agentik_sdk::model::{Model, ProviderConfig, ProviderType};
use agentik_sdk::provider::ProviderPreset;
use agentik_sdk::provider::zai::ZaiProvider;
use agentik_sdk::types::AgentEvent;
use agentik_sdk::types::messages::ContentBlock;
use arc_swap::ArcSwapOption;
use bib_base::{BibShared, bib_all_registrations, sanitize_figure_name, scan_image_refs};
use std::sync::Arc;
use std::time::Duration;

const ZAI_ANTHROPIC_BASE: &str = "https://open.bigmodel.cn/api/anthropic";
const PDF_ATTENTION: &str = "https://arxiv.org/pdf/1706.03762";
const PDF_BERT: &str = "https://arxiv.org/pdf/1810.04805";
const ARTICLE_ID: &str = "doi:10.1000/mineru-e2e";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env and scrub tool-image overrides BEFORE the async runtime
    // spawns worker threads — env mutation is only sound while the process
    // is single-threaded, which is why edition 2024 marks it unsafe.
    for candidate in ["../../.env", "./.env"] {
        if let Ok(text) = std::fs::read_to_string(candidate) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if std::env::var_os(k).is_none() {
                        // SAFETY: runs before the tokio runtime (and any
                        // other thread) exists — the process is still
                        // single-threaded here.
                        unsafe { std::env::set_var(k.trim(), v.trim()) };
                    }
                }
            }
        }
    }
    // SAFETY: see above. Exercise the default `auto` mode.
    unsafe { std::env::remove_var("AGENTIK_TOOL_IMAGES") };
    if std::env::var_os("MINERU_API_KEY").is_none() {
        return Err("MINERU_API_KEY not found in .env or environment".into());
    }
    // The MinerU key is cloud-mineru-only; the Zhipu LLM key (glm-5.3-flash /
    // glm-5-turbo on the Anthropic-compatible endpoint) is exported separately.
    let llm_key = std::env::var("ANTHROPIC_AUTH_TOKEN")
        .map_err(|_| "ANTHROPIC_AUTH_TOKEN not set (Zhipu LLM key)")?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(llm_key))
}

async fn run(key: String) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // ── scratch environment ──
    let root = std::env::temp_dir().join(format!("autonomics-mineru-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    println!("[setup] scratch dir: {}", root.display());
    let config = runtime::RuntimeConfig::builder()
        .data_dir(root.join("data"))
        .state_dir(root.join("state"))
        .build();
    let storage = runtime::host::bibliography_file_storage(&config)
        .map_err(|e| format!("vfs storage: {e}"))?;

    let shared = BibShared::open(root.join("bib.db"))
        .await?
        .with_file_storage(storage.clone());
    let server = tui_http::start(tui_http::api_router(shared.clone()), "127.0.0.1:0").await?;
    let base = format!("http://{}/api/v1/bib", server.addr());
    println!("[setup] api server listening: {}", server.addr());

    // ── 1. download PDFs ──
    let pdf1 = download(&shared, PDF_ATTENTION, "attention.pdf").await?;
    let pdf2 = download(&shared, PDF_BERT, "bert.pdf").await?;

    // ── 2. create article + upload the first PDF ──
    let created: serde_json::Value = serde_json::from_str(
        &shared
            .http
            .post(format!("{base}/articles"))
            .header("Content-Type", "application/json")
            .body(r#"{ "title": "MinerU figure e2e", "doi": "10.1000/mineru-e2e" }"#)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?,
    )?;
    assert_eq!(created["article"]["id"].as_str(), Some(ARTICLE_ID));
    upload_fulltext(&shared, &base, ARTICLE_ID, "attention.pdf", &pdf1).await?;

    // ── 3. wait for MinerU cloud extraction ──
    let ft = poll_extraction(&shared, &base, ARTICLE_ID, Duration::from_secs(600)).await?;
    assert_eq!(
        ft["text_format"].as_str(),
        Some("markdown"),
        "MinerU should produce markdown"
    );
    let markdown = ft["text_content"].as_str().expect("markdown body");
    let refs1: Vec<String> = scan_image_refs(markdown);
    println!(
        "[extract] markdown {} chars, {} figure refs",
        markdown.len(),
        refs1.len()
    );
    assert!(
        !refs1.is_empty(),
        "source PDF has figures; none referenced in markdown"
    );

    // ── 4. image route serves real bytes ──
    let name1 = sanitize_figure_name(&refs1[0]).expect("first ref sanitizes");
    let (status, content_type, bytes) = get_image(&shared, &base, ARTICLE_ID, &name1).await?;
    assert_eq!(status, 200, "image route should return 200");
    assert!(
        content_type.starts_with("image/"),
        "content-type should be image/*, got {content_type}"
    );
    let magic_ok = match name1.rsplit('.').next().unwrap_or_default() {
        "png" => bytes.starts_with(&[0x89, b'P', b'N', b'G']),
        _ => bytes.starts_with(&[0xFF, 0xD8, 0xFF]), // jpg/jpeg
    };
    assert!(
        magic_ok,
        "served bytes should carry the {name1} magic header"
    );
    println!(
        "[route] GET {name1} → {status} {content_type} {} bytes (magic ok)",
        bytes.len()
    );

    // ── 5. vision agent reads the figure ──
    let (vision_answer, vision_tools, vision_errors) = run_agent(
        &shared,
        &storage,
        &key,
        agentik_sdk::provider::zai::MODEL_GLM_5_3_FLASH,
        ARTICLE_ID,
        &refs1[0],
    )
    .await?;
    println!("[vision] tools called: {vision_tools:?}");
    println!("[vision] answer: {vision_answer}");
    assert!(
        vision_tools.iter().any(|t| t == "bib_read_figure"),
        "vision agent should call bib_read_figure"
    );
    assert!(
        vision_errors.is_empty(),
        "vision agent errors: {vision_errors:?}"
    );
    assert!(
        !vision_answer.trim().is_empty(),
        "vision agent should describe the figure"
    );

    // ── 6. replace upload → old figures deleted, new ones served ──
    upload_fulltext(&shared, &base, ARTICLE_ID, "bert.pdf", &pdf2).await?;
    let ft2 = poll_extraction(&shared, &base, ARTICLE_ID, Duration::from_secs(600)).await?;
    let markdown2 = ft2["text_content"].as_str().expect("markdown body 2");
    let refs2: Vec<String> = scan_image_refs(markdown2);
    println!(
        "[reextract] second markdown {} chars, {} figure refs",
        markdown2.len(),
        refs2.len()
    );
    assert!(!refs2.is_empty(), "second PDF also has figures");
    let stale: Vec<&String> = refs1.iter().filter(|r| !refs2.contains(r)).collect();
    assert!(
        !stale.is_empty(),
        "the two PDFs should reference different figures"
    );
    for old in &stale {
        let old_name = sanitize_figure_name(old).expect("old ref sanitizes");
        let (status, _, _) = get_image(&shared, &base, ARTICLE_ID, &old_name).await?;
        assert_eq!(
            status, 404,
            "replaced figure {old_name} should be gone, got {status}"
        );
    }
    println!(
        "[reextract] {} replaced figure object(s) removed from VFS",
        stale.len()
    );

    // ── 7. text-only control on the new figures ──
    let (text_answer, text_tools, text_errors) = run_agent(
        &shared,
        &storage,
        &key,
        "glm-5-turbo",
        ARTICLE_ID,
        &refs2[0],
    )
    .await?;
    println!("[text]   tools called: {text_tools:?}");
    println!("[text]   answer: {text_answer}");
    assert!(
        text_tools.iter().any(|t| t == "bib_read_figure"),
        "text agent should still call bib_read_figure (text summary path)"
    );
    assert!(
        text_errors.is_empty(),
        "text control errors: {text_errors:?}"
    );

    println!(
        "\n=== e2e OK === everything checked; scratch dir left at {}",
        root.display()
    );
    server
        .shutdown()
        .await
        .map_err(|e| format!("server shutdown: {e}"))?;
    Ok(())
}

// ── helpers ─────────────────────────────────────────────────────────

async fn download(
    shared: &BibShared,
    url: &str,
    label: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = shared
        .http
        .get(url)
        .header("User-Agent", "autonomics-e2e/0.1")
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    assert!(bytes.starts_with(b"%PDF-"), "{label} is not a PDF");
    println!("[download] {label}: {} bytes", bytes.len());
    Ok(bytes.to_vec())
}

/// POST the PDF as a hand-rolled `multipart/form-data` body (field `file`).
async fn upload_fulltext(
    shared: &BibShared,
    base: &str,
    article_id: &str,
    filename: &str,
    pdf: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let boundary = "autonomicsE2EBoundary";
    let mut body = Vec::with_capacity(pdf.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: application/pdf\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(pdf);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let status = shared
        .http
        .post(format!("{}/articles/{}/fulltext", base, enc(article_id)))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await?
        .status();
    assert_eq!(status, 200, "upload of {filename} failed");
    println!("[upload] {filename} accepted");
    Ok(())
}

/// Poll the fulltext row until extraction reaches a terminal state.
///
/// Transient server 5xx responses (e.g. DB lock contention while the
/// extraction task records its status) are tolerated; a genuinely failed
/// extraction is retried once via the reextract endpoint.
async fn poll_extraction(
    shared: &BibShared,
    base: &str,
    article_id: &str,
    timeout: Duration,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut consecutive_errors = 0u32;
    let mut retried = false;
    loop {
        let resp = shared
            .http
            .get(format!("{}/articles/{}/fulltext", base, enc(article_id)))
            .send()
            .await?;
        if !resp.status().is_success() {
            consecutive_errors += 1;
            println!(
                "[extract] poll got HTTP {}, attempt {consecutive_errors}",
                resp.status()
            );
            if consecutive_errors >= 6 {
                return Err("fulltext poll kept failing".into());
            }
        } else {
            consecutive_errors = 0;
            let body: serde_json::Value = serde_json::from_str(&resp.text().await?)?;
            let ft = body["fulltext"].clone();
            match ft["extract_status"].as_str() {
                Some("done") => {
                    println!("[extract] status=done");
                    return Ok(ft);
                }
                Some("failed") if !retried => {
                    retried = true;
                    println!("[extract] status=failed once, retrying via reextract");
                    let status = shared
                        .http
                        .post(format!(
                            "{}/articles/{}/fulltext/reextract",
                            base,
                            enc(article_id)
                        ))
                        .send()
                        .await?
                        .status();
                    assert_eq!(status, 200, "reextract request failed");
                }
                Some("failed") => {
                    return Err(format!("extraction failed twice: {ft}").into());
                }
                other => println!("[extract] status={other:?}, waiting…"),
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("extraction timed out".into());
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn get_image(
    shared: &BibShared,
    base: &str,
    article_id: &str,
    name: &str,
) -> Result<(u16, String, Vec<u8>), Box<dyn std::error::Error>> {
    let resp = shared
        .http
        .get(format!(
            "{}/articles/{}/fulltext/images/{}",
            base,
            enc(article_id),
            enc(name)
        ))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Ok((status, content_type, resp.bytes().await?.to_vec()))
}

/// Run one agent turn against `model_id` and collect its answer, tool calls,
/// and error events. The agent shares the scratch bib DB + VFS storage.
async fn run_agent(
    shared: &BibShared,
    storage: &Arc<vfs::OpendalFileStorage>,
    key: &str,
    model_id: &str,
    article_id: &str,
    figure_ref: &str,
) -> Result<(String, Vec<String>, Vec<String>), Box<dyn std::error::Error>> {
    let provider = ProviderConfig::new(
        "zai",
        ProviderType::Zai,
        ZAI_ANTHROPIC_BASE,
        key,
        ZaiProvider::default_auth_method(),
    );
    let mut info = ZaiProvider::preset_models()
        .into_iter()
        .find(|m| m.model_name == model_id)
        .ok_or_else(|| format!("model {model_id} not in zai catalogue"))?;
    info.provider_id = provider.id;
    // Zhipu caps glm-4v-flash output at 1024 tokens (error 1210); the request
    // layer sends max_output_tokens verbatim, so clamp the catalogue's 8000.
    // (The e2e uses glm-5.3-flash, which accepts the full 131072.)
    if model_id == agentik_sdk::provider::zai::MODEL_GLM_4V_FLASH {
        info.max_output_tokens = 1024;
    }
    let model = Model::new(info, &provider)?;

    let tools = bib_all_registrations(
        shared.bib.clone(),
        shared.gateway.clone(),
        None,
        Some(storage.clone()),
    );
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut agent = Agent::builder()
        .with_name("figure-e2e")
        .with_model(Arc::new(ArcSwapOption::from_pointee(Some(model))))
        .with_tools(tools)
        .with_agent_event_tx(event_tx)
        .build()
        .await?;
    let internal_tx = agent.internal_event_tx().clone();
    let agent_task = tokio::spawn(async move {
        agent.run().await;
    });

    let prompt = format!(
        "Use the bib_read_figure tool with article_id \"{article_id}\" and figure \
         \"{figure_ref}\" to view a figure of the paper, then describe in two or \
         three sentences what is actually depicted in the image."
    );
    internal_tx.send(InternalEvent::MessageInject {
        content: vec![ContentBlock::Text { text: prompt }],
        from_user: true,
        delegation_id: None,
    })?;

    let mut answers: Vec<String> = Vec::new();
    let mut tool_calls: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let deadline = Duration::from_secs(300);
    loop {
        let event = tokio::time::timeout(deadline, event_rx.recv())
            .await
            .map_err(|_| format!("agent turn timed out after {deadline:?}"))?
            .ok_or("agent event channel closed")?;
        match event {
            AgentEvent::LlmResponse(text) => answers.push(text),
            AgentEvent::ToolCall { name, .. } => tool_calls.push(name),
            // Workflow failure is terminal for the turn (no Done follows).
            AgentEvent::Error(e) => {
                errors.push(e);
                break;
            }
            AgentEvent::Done => break,
            _ => {}
        }
    }
    agent_task.abort();
    Ok((answers.join("\n"), tool_calls, errors))
}

/// Percent-encode just the characters that appear in article ids / names
/// (`:` and `/`); figure names are `[A-Za-z0-9._-]` and pass through.
fn enc(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}
