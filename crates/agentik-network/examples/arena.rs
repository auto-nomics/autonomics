//! Arena example — run a 2-agent adversarial manuscript review loop.
//!
//! Demonstrates the persistent mutable model:
//! - [`RuntimeHost`] owns [`AgentNetwork`] as a persistent topology manager.
//! - Topology is built imperatively via `host.add_node()` / `host.connect()`.
//! - Event loop driven by `host.step()`.
//!
//! ## Usage
//!
//! ```sh
//! export DEEPSEEK_API_KEY="sk-..."
//! cargo run --example arena -- -t "Write a 200-word abstract about the \
//!     utility of LD score regression" -m 3
//! ```

use std::sync::Arc;

use agentik_core::AgentProfile;
use agentik_network::{EdgeTrigger, RoutingAction, TerminationSpec};
use agentik_sdk::model::{Model, ProviderConfig, ProviderType};
use agentik_sdk::AuthMethod;
use arc_swap::ArcSwapOption;
use clap::Parser;
use runtime::config::RuntimeConfig;
use runtime::RuntimeHost;

/// CLI arguments for the arena example.
#[derive(Parser)]
struct Args {
    #[arg(short, long, default_value = "\
        Write a 300-word manuscript abstract on the role of polygenic risk \
        scores in precision medicine, covering methodology, current \
        applications, and limitations.")]
    topic: String,

    #[arg(short, long, default_value_t = 3)]
    max_rounds: usize,

    #[arg(short, long, default_value = "deepseek")]
    provider: String,

    #[arg(long, default_value = "deepseek-chat")]
    model: String,

    #[arg(long, default_value = "VERDICT: ACCEPT")]
    accept: String,
}

const BAR: &str = "══════════════════════════════════════════════════════════════════";
const DASH: &str = "──────────────────────────────────────────────────────────────────";

fn writer_profile() -> AgentProfile {
    AgentProfile {
        id: uuid::Uuid::new_v4(),
        name: "arena-writer".into(),
        description: "Manuscript writer.".into(),
        agent_identity: "\
            You are a biomedical researcher writing a manuscript. \
            You will receive reviewer feedback and must revise your work. \
            Your goal is to produce a manuscript rigorous enough to be \
            accepted by a top journal. \
            Write in clear, precise academic prose.".into(),
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

fn reviewer_profile() -> AgentProfile {
    AgentProfile {
        id: uuid::Uuid::new_v4(),
        name: "arena-reviewer".into(),
        description: "Peer reviewer.".into(),
        agent_identity: "\
            You are a rigorous peer reviewer for a top biomedical journal. \
            If the manuscript meets your standards, end your review with \
            exactly: VERDICT: ACCEPT \
            Otherwise, provide specific actionable criticism and end with: \
            VERDICT: REJECT".into(),
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

fn build_model(provider: &str, model_name: &str) -> Result<Model, Box<dyn std::error::Error>> {
    use agentik_sdk::provider::registry;
    let provider_type = ProviderType::from(provider);
    let api_key = match provider_type {
        ProviderType::Deepseek => std::env::var("DEEPSEEK_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| "DEEPSEEK_API_KEY not set")?,
        ProviderType::Moonshot => std::env::var("MOONSHOT_API_KEY").map_err(|_| "MOONSHOT_API_KEY not set")?,
        ProviderType::Minimax => std::env::var("MINIMAX_API_KEY").map_err(|_| "MINIMAX_API_KEY not set")?,
        ProviderType::Mimo => std::env::var("MIMO_API_KEY").map_err(|_| "MIMO_API_KEY not set")?,
        ProviderType::Zai => std::env::var("ZAI_API_KEY").map_err(|_| "ZAI_API_KEY not set")?,
        ProviderType::Sensenova => std::env::var("SENSENOVA_API_KEY").map_err(|_| "SENSENOVA_API_KEY not set")?,
        ProviderType::Custom(ref name) => {
            let env = format!("{}_API_KEY", name.to_uppercase());
            std::env::var(&env).map_err(|_| format!("{env} not set"))?
        }
    };
    let base_url = registry::default_base_url(&provider_type).ok_or("no default base URL")?.to_string();
    let auth_method = registry::default_auth_method(&provider_type);
    let preset_models = registry::preset_models(&provider_type).ok_or("no preset models")?;
    let mut model_info = preset_models
        .into_iter()
        .find(|m| m.model_name == model_name)
        .ok_or_else(|| format!("model '{model_name}' not in preset catalog"))?;
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
                .add_directive("runtime=info".parse()?),
        )
        .init();

    let args = Args::parse();

    println!("{BAR}");
    println!("          🏟️  Adversarial Arena Started");
    println!("{BAR}");
    println!("  Provider: {}  Model: {}  Max rounds: {}", args.provider, args.model, args.max_rounds);

    // ── Build model + host ──────────────────────────────────
    let model = build_model(&args.provider, &args.model)?;
    let global_model = Arc::new(ArcSwapOption::from_pointee(Some(model)));

    let config = RuntimeConfig::default();
    let mut host = RuntimeHost::open(&config).await?;

    // ── Build topology imperatively ─────────────────────────
    // RuntimeHost owns the persistent AgentNetwork internally.
    host.add_node_with_prompt("writer", "arena-writer", &args.topic)?;
    host.add_node("reviewer", "arena-reviewer")?;

    // writer → reviewer (submit manuscript)
    host.connect("writer", "reviewer", EdgeTrigger::OnDone)?;
    // reviewer → writer (return feedback)
    host.connect("reviewer", "writer", EdgeTrigger::OnDone)?;

    host.set_termination(TerminationSpec::Any {
        specs: vec![
            TerminationSpec::Condition {
                node: "reviewer".into(),
                pattern: args.accept.clone(),
            },
            TerminationSpec::MaxRounds {
                max: args.max_rounds * 2,
            },
        ],
    });

    // ── Spawn agents and register them ──────────────────────
    let wp = writer_profile();
    let rp = reviewer_profile();

    host.spawn_and_register("writer", &wp, global_model.clone(), None).await?;
    host.spawn_and_register("reviewer", &rp, global_model.clone(), None).await?;

    // ── Inject initial prompts ──────────────────────────────
    host.inject_initial_prompts();

    println!("\n🚀 Arena started. Topology: writer ↔ reviewer\n");

    // ── Event loop (driven by host.step()) ──────────────────
    use agentik_sdk::types::AgentEvent;

    while !host.network().is_finished() {
        let Some((agent_name, event, actions)) = host.step().await else {
            break;
        };

        if let AgentEvent::LlmResponse(ref text) = event {
            print!("[{agent_name}] {text}");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }

        for action in &actions {
            match action {
                RoutingAction::Forward { to, .. } => {
                    println!("\n{DASH}");
                    println!("📨 {agent_name} → {to}");
                }
                RoutingAction::Finished { reason } => {
                    println!("\n{BAR}");
                    println!("🏁 Finished: {reason:?}");
                    println!("   Rounds: {}", host.network().rounds());
                }
            }
        }
    }

    host.shutdown_all_agents();
    Ok(())
}
