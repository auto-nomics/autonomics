//! Worker for the real SIGKILL qualification driver. Uses a controlled local
//! Ollama HTTP fixture; no semantic relevance or real model claim is made.
use bib_base::{
    BibBase, SearchEngineMode,
    semantic_index::SemanticProfile,
    semantic_service::{
        SemanticConfig, SemanticMaintenanceAction, SemanticMaintenanceRequest, SemanticPhase,
    },
};
use bib_types::Article;
use std::{io::Write, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 || !matches!(args[0].as_str(), "start" | "recover") {
        return Err("usage: semantic_process_probe start|recover DATABASE LOCAL_ORIGIN".into());
    }
    let library = BibBase::open_with_search_engine(&args[1], SearchEngineMode::Tantivy).await?;
    if args[0] == "start" {
        let papers = (0..20)
            .map(|i| {
                let mut paper = Article::new(
                    format!("probe-{i:02}"),
                    format!("Recovery fixture paper {i}"),
                );
                paper.abstract_text =
                    Some(format!("Controlled protocol fixture abstract number {i}."));
                paper
            })
            .collect::<Vec<_>>();
        library.upsert_articles(&papers).await?;
        library
            .configure_semantic_search(SemanticConfig {
                enabled: true,
                origin: args[2].clone(),
                model: "fixture:latest".into(),
                model_digest: "a".repeat(64),
                profile: SemanticProfile {
                    dimensions: 2,
                    ..Default::default()
                },
                ..Default::default()
            })
            .await?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut started = args[0] == "recover";
    let mut previous = String::new();
    loop {
        let status = library.semantic_search_status()?;
        let serialized = serde_json::to_string(&status)?;
        if serialized != previous {
            println!("{serialized}");
            std::io::stdout().flush()?;
            previous = serialized;
        }
        if status.phase == SemanticPhase::Failed {
            return Err(status
                .last_error
                .unwrap_or_else(|| "semantic worker failed".into())
                .into());
        }
        if !started && status.phase == SemanticPhase::Ready {
            library
                .maintain_semantic_search(SemanticMaintenanceRequest {
                    action: SemanticMaintenanceAction::Repair,
                    expected_configuration_generation: status.configuration_generation,
                    request_id: uuid::Uuid::new_v4().to_string(),
                })
                .await?;
            started = true;
        } else if args[0] == "recover"
            && status.phase == SemanticPhase::Ready
            && status
                .maintenance
                .as_ref()
                .is_some_and(|job| job.state == "complete")
        {
            assert_eq!(status.indexed_chunks, 20);
            let page = library
                .search_v2(bib_types::SearchRequestV2 {
                    mode: bib_types::SearchMode::Hybrid,
                    semantic_query: Some("scientific evidence".into()),
                    ..Default::default()
                })
                .await?;
            assert_eq!(page.total, 20);
            println!(
                "{}",
                serde_json::json!({"event":"recovery_complete","status":status,"query_total":page.total})
            );
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("semantic process qualification timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
