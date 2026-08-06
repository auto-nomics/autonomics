//! Arena example — run a 2-agent adversarial manuscript review loop.
//!
//! ## Usage
//!
//! ```sh
//! # Set your API key (e.g. DeepSeek)
//! export DEEPSEEK_API_KEY="sk-..."
//!
//! # Run the arena
//! cargo run --example arena -- -t "Write a 200-word abstract about the \
//!     utility of LD score regression for estimating heritability from GWAS \
//!     summary statistics" -m 3
//! ```
//!
//! ## What happens
//!
//! 1. A "writer" agent drafts a manuscript from the topic prompt.
//! 2. A "reviewer" agent critiques it.
//! 3. If the reviewer writes `VERDICT: ACCEPT`, the loop ends.
//! 4. Otherwise the writer revises and resubmits.
//! 5. After `max_rounds` (default 3) revisions without acceptance, the
//!    loop terminates.

use std::collections::HashMap;
use std::sync::Arc;

use agentik_core::AgentProfile;
use agentik_network::{AgentNetwork, NetworkEvent, NetworkOutcome, presets};
use agentik_sdk::model::{Model, ProviderConfig, ProviderType};
use agentik_sdk::AuthMethod;
use arc_swap::ArcSwapOption;
use clap::Parser;
use runtime::RuntimeHost;
use runtime::config::RuntimeConfig;

/// CLI arguments for the arena example.
#[derive(Parser)]
struct Args {
    /// Topic / writing prompt for the manuscript.
    #[arg(short, long, default_value = "\
        Write a 300-word manuscript abstract on the role of polygenic risk \
        scores in precision medicine, covering methodology, current \
        applications, and limitations.")]
    topic: String,

    /// Maximum revision rounds before forced termination.
    #[arg(short, long, default_value_t = 3)]
    max_rounds: usize,

    /// Provider name (e.g. "deepseek").
    #[arg(short, long, default_value = "deepseek")]
    provider: String,

    /// Model name (e.g. "deepseek-chat").
    #[arg(long, default_value = "deepseek-chat")]
    model: String,

    /// Accept pattern — reviewer must include this to signal acceptance.
    #[arg(long, default_value = "VERDICT: ACCEPT")]
    accept: String,
}

const BAR: &str = "══════════════════════════════════════════════════════════════════";
const DASH: &str = "──────────────────────────────────────────────────────────────────";

/// Build a writer profile.
fn writer_profile() -> AgentProfile {
    AgentProfile {
        id: uuid::Uuid::new_v4(),
        name: "arena-writer".into(),
        description: "Manuscript writer for the adversarial arena.".into(),
        agent_identity: "\
            You are a biomedical researcher writing a manuscript. \
            You will receive reviewer feedback and must revise your work. \
            Your goal is to produce a manuscript rigorous enough to be \
            accepted by a top journal. \
            Write in clear, precise academic prose. Structure your \
            submission with clear sections.".into(),
        system_prompt: None,
        enable_bibliography: true,
        enable_opengwas: false,
        enable_opentargets: false,
        enable_gwascatalog: false,
        enable_iceberg: false,
        enable_dag_history: false,
        preferred_model: None,
        created_at: 0,
        updated_at: 0,
    }
}

/// Build a reviewer profile.
fn reviewer_profile() -> AgentProfile {
    AgentProfile {
        id: uuid::Uuid::new_v4(),
        name: "arena-reviewer".into(),
        description: "Peer reviewer for the adversarial arena.".into(),
        agent_identity: "\
            You are a rigorous peer reviewer for a top biomedical journal. \
            Analyze the manuscript for: \
            (1) Methodological soundness, \
            (2) Clarity and structure, \
            (3) Appropriate use of citations, \
            (4) Statistical rigor, \
            (5) Overall impact. \
            \
            If the manuscript meets your standards, end your review with \
            exactly: VERDICT: ACCEPT \
            \
            If it needs revision, provide specific, actionable criticism \
            and end with: VERDICT: REJECT".into(),
        system_prompt: None,
        enable_bibliography: true,
        enable_opengwas: false,
        enable_opentargets: false,
        enable_gwascatalog: false,
        enable_iceberg: false,
        enable_dag_history: false,
        preferred_model: None,
        created_at: 0,
        updated_at: 0,
    }
}

/// Build a Model from environment variables and CLI args.
fn build_model(
    provider: &str,
    model_name: &str,
) -> Result<Model, Box<dyn std::error::Error>> {
    use agentik_sdk::provider::registry;

    let provider_type = ProviderType::from(provider);

    // Resolve API key from env var based on provider type.
    let api_key = match provider_type {
        ProviderType::Deepseek => std::env::var("DEEPSEEK_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| "DEEPSEEK_API_KEY not set")?,
        ProviderType::Moonshot => std::env::var("MOONSHOT_API_KEY")
            .map_err(|_| "MOONSHOT_API_KEY not set")?,
        ProviderType::Minimax => std::env::var("MINIMAX_API_KEY")
            .map_err(|_| "MINIMAX_API_KEY not set")?,
        ProviderType::Mimo => std::env::var("MIMO_API_KEY")
            .map_err(|_| "MIMO_API_KEY not set")?,
        ProviderType::Zai => std::env::var("ZAI_API_KEY")
            .map_err(|_| "ZAI_API_KEY not set")?,
        ProviderType::Sensenova => std::env::var("SENSENOVA_API_KEY")
            .map_err(|_| "SENSENOVA_API_KEY not set")?,
        ProviderType::Custom(ref name) => {
            let env = format!("{}_API_KEY", name.to_uppercase());
            std::env::var(&env).map_err(|_| format!("{env} not set"))?
        }
    };

    let base_url = registry::default_base_url(&provider_type)
        .ok_or("no default base URL")?
        .to_string();
    let auth_method = registry::default_auth_method(&provider_type);

    let preset_models =
        registry::preset_models(&provider_type).ok_or("no preset models")?;
    let mut model_info = preset_models
        .into_iter()
        .find(|m| m.model_name == model_name)
        .ok_or_else(|| {
            format!("model '{model_name}' not in preset catalog for {provider}")
        })?;

    let provider_config = ProviderConfig {
        id: uuid::Uuid::nil(),
        name: provider.to_string(),
        provider_type,
        base_url,
        api_key,
        auth_method,
    };
    model_info.provider_id = provider_config.id;

    Ok(Model::new(model_info, &provider_config)?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("agentik_network=info".parse()?),
        )
        .init();

    let args = Args::parse();

    // ── Banner ───────────────────────────────────────────────
    println!("{BAR}");
    println!("          🏟️  Adversarial Arena Started");
    println!("{BAR}");
    println!("  Provider:   {}", args.provider);
    println!("  Model:      {}", args.model);
    println!("  Max rounds: {}", args.max_rounds);
    let topic_preview = if args.topic.len() > 80 {
        format!("{}...", &args.topic[..80])
    } else {
        args.topic.clone()
    };
    println!("  Topic:      {topic_preview}");
    println!();

    // ── Build model ──────────────────────────────────────────
    let model = build_model(&args.provider, &args.model)?;
    let global_model = Arc::new(ArcSwapOption::from_pointee(Some(model)));

    // ── Open runtime host ────────────────────────────────────
    let config = RuntimeConfig::default();
    let host = RuntimeHost::open(&config).await?;

    // ── Build profiles map ───────────────────────────────────
    let mut profiles = HashMap::new();
    let wp = writer_profile();
    let rp = reviewer_profile();
    profiles.insert(wp.name.clone(), wp);
    profiles.insert(rp.name.clone(), rp);

    // ── Build network spec ───────────────────────────────────
    let spec = presets::arena(
        &args.topic,
        args.max_rounds,
        "arena-writer",
        "arena-reviewer",
        Some(&args.accept),
    );

    // ── Observer channel ─────────────────────────────────────
    let (observer_tx, mut observer_rx) =
        tokio::sync::mpsc::unbounded_channel::<NetworkEvent>();

    // Spawn an observer task that prints events as they arrive.
    let printer = tokio::spawn(async move {
        while let Some(ev) = observer_rx.recv().await {
            match ev {
                NetworkEvent::Started { name, node_count } => {
                    println!("📋 Network '{name}' started with {node_count} nodes.");
                }
                NetworkEvent::MessageRouted { from, to, round } => {
                    println!("\n{DASH}");
                    println!("📨 Round {round}: {from} → {to}");
                }
                NetworkEvent::AgentText { node, text } => {
                    print!("[{node}] {text}");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                NetworkEvent::ToolCall { node, tool } => {
                    println!("\n  🔧 [{node}] calling tool: {tool}");
                }
                NetworkEvent::AgentError { node, error } => {
                    eprintln!("\n  ❌ [{node}] error: {error}");
                }
                NetworkEvent::Finished { reason, rounds, final_node } => {
                    println!("\n{BAR}");
                    println!("🏁 Network finished after {rounds} rounds.");
                    println!("   Reason: {reason}");
                    if let Some(node) = final_node {
                        println!("   Final node: {node}");
                    }
                }
                _ => {}
            }
        }
    });

    // ── Build + run ──────────────────────────────────────────
    let network =
        AgentNetwork::build(spec, &host, global_model, profiles, observer_tx).await?;

    println!("\n🚀 Network built. Starting the adversarial loop...\n");

    let outcome = network.run().await;

    // ── Report outcome ───────────────────────────────────────
    match &outcome {
        NetworkOutcome::Terminated {
            reason,
            rounds,
            final_node,
            final_output,
        } => {
            println!("\n{BAR}");
            println!("🏆 TERMINATED");
            println!("   Rounds:     {rounds}");
            println!("   Reason:     {reason:?}");
            if let Some(node) = final_node {
                println!("   Final node: {node}");
            }
            if let Some(output) = final_output {
                println!("\n📜 Final output from {}:", final_node.as_deref().unwrap_or("?"));
                println!("{DASH}");
                let preview = if output.len() > 1000 {
                    format!("{}...[truncated]", &output[..1000])
                } else {
                    output.clone()
                };
                println!("{preview}");
            }
        }
        NetworkOutcome::Completed { rounds } => {
            println!("\n✅ All agents finished after {rounds} rounds.");
        }
    }

    // Wait for the printer to drain.
    let _ = printer.await;

    Ok(())
}
