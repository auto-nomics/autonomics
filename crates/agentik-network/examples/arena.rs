//! Arena example — run a 2-agent adversarial manuscript review loop.
//!
//! Demonstrates the separation of concerns:
//! - [`RuntimeHost`] + [`AgentRegistry`] — agent lifecycle + transport.
//! - [`AgentNetwork`] — pure topology routing (no I/O).
//!
//! ## Usage
//!
//! ```sh
//! export DEEPSEEK_API_KEY="sk-..."
//! cargo run --example arena -- -t "Write a 200-word abstract about the \
//!     utility of LD score regression" -m 3
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use agentik_core::AgentProfile;
use agentik_network::presets;
use agentik_network::{AgentNetwork, RoutingAction};
use agentik_sdk::model::{Model, ProviderConfig, ProviderType};
use agentik_sdk::AuthMethod;
use arc_swap::ArcSwapOption;
use clap::Parser;
use runtime::config::RuntimeConfig;
use runtime::{AgentRegistry, RuntimeHost};

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

    // ── Build model + host + registry ────────────────────────
    let model = build_model(&args.provider, &args.model)?;
    let global_model = Arc::new(ArcSwapOption::from_pointee(Some(model)));

    let config = RuntimeConfig::default();
    let host = RuntimeHost::open(&config).await?;
    let mut registry = AgentRegistry::new();

    // ── Build profiles ───────────────────────────────────────
    let wp = writer_profile();
    let rp = reviewer_profile();
    let profiles: HashMap<String, AgentProfile> = [
        (wp.name.clone(), wp),
        (rp.name.clone(), rp),
    ]
    .into_iter()
    .collect();

    // ── Build network spec (pure topology) ───────────────────
    let spec = presets::arena(
        &args.topic,
        args.max_rounds,
        "arena-writer",
        "arena-reviewer",
        Some(&args.accept),
    );

    let mut network = AgentNetwork::new(spec)?;

    // ── Spawn agents and register with the registry ──────────
    for node_spec in network.spec().nodes.iter() {
        let profile = profiles
            .get(&node_spec.profile)
            .ok_or_else(|| format!("profile not found: {}", node_spec.profile))?;
        let handle = host
            .spawn_agent(
                &node_spec.name,
                profile,
                global_model.clone(),
                None,
            )
            .await?;
        registry.register(handle);
        println!("  Spawned agent: {}", node_spec.name);
    }

    // ── Inject initial prompts ───────────────────────────────
    for (node, prompt) in network.initial_messages() {
        println!("\n🚀 Injecting prompt to {node}...");
        registry.send_to(&node, prompt);
    }

    println!("\n{BAR}");
    println!("Starting the adversarial loop...\n");

    // ── Event loop ───────────────────────────────────────────
    // The host drives the loop; AgentNetwork is a pure state machine.
    let mut final_node = String::new();

    while let Some((agent_name, event)) = registry.recv_any().await {
        use agentik_sdk::types::AgentEvent;

        // Print streaming text.
        if let AgentEvent::LlmResponse(ref text) = event {
            print!("[{agent_name}] {text}");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }

        // Feed event to the routing state machine.
        let actions = network.process_event(&agent_name, &event);

        for action in actions {
            match action {
                RoutingAction::Forward { to, message } => {
                    println!("\n{DASH}");
                    println!("📨 {agent_name} → {to}");
                    registry.send_to(&to, message);
                }
                RoutingAction::Finished { reason } => {
                    final_node = agent_name.clone();
                    if let AgentEvent::Done = event {
                        // The Done event's response was already processed.
                    }
                    println!("\n{BAR}");
                    println!("🏁 Network finished.");
                    println!("   Reason: {reason:?}");
                    println!("   Rounds: {}", network.rounds());
                }
            }
        }

        if network.is_finished() {
            break;
        }
    }

    // ── Cleanup ──────────────────────────────────────────────
    registry.shutdown_all();

    if !final_node.is_empty() {
        println!("   Final node: {final_node}");
    }

    Ok(())
}
