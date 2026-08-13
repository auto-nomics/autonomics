use std::sync::Arc;

use agentik_core::agent::{Agent, AgentConfig, InternalEvent};
use agentik_sdk::ContentBlock;
use agentik_sdk::http::auth::AuthMethod;
use agentik_sdk::model::ProviderConfig;
use agentik_sdk::provider::mimo::MODEL_MIMO_V2_5;
use agentik_sdk::{
    model::Model,
    provider::mimo::{MimoEndpoint, MimoProvider, TokenPlanRegion},
    types::AgentEvent,
};

fn build_mimo_model() -> Model {
    let api_key = std::env::var("MIMO_API_KEY").expect("MIMO_API_KEY not set");
    let endpoint = MimoEndpoint::TokenPlan(TokenPlanRegion::China);

    let provider = ProviderConfig::new(
        "mimo-cn",
        "mimo",
        endpoint.base_url().to_string(),
        api_key,
        AuthMethod::Anthropic,
    );

    let mut model_info = MimoProvider::preset_models()
        .into_iter()
        .find(|m| m.model_name == MODEL_MIMO_V2_5)
        .expect("preset model not found");
    model_info.provider_id = provider.id;

    Model::new(model_info, &provider).expect("failed to build mimo model")
}

#[tokio::test]
#[ignore]
async fn test_agent_basic_workflow_with_mimo() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

    let mut agent = Agent::builder()
        .with_model(Arc::new(arc_swap::ArcSwapOption::from_pointee(Some(
            build_mimo_model(),
        ))))
        .with_system_prompt_section(
            "You are a helpful assistant. Keep responses very short (one sentence).",
        )
        .with_config(AgentConfig {
            max_iterations: 3,
            max_retries: 1,
        })
        .build()
        .await
        .expect("failed to build agent");

    agent.set_agent_event_tx(tx);
    let internal_tx = agent.internal_event_tx();
    let handle = tokio::spawn(async move {
        let _ = agent.run().await;
    });

    // `run()` is event-driven: it blocks on the internal channel and only runs
    // a session once a `MessageInject` arrives. Seed messages alone won't start it.
    let _ = internal_tx.send(InternalEvent::MessageInject {
        content: vec![ContentBlock::Text {
            text: "Say exactly: hello world".into(),
        }],
        from_user: false,
    });

    let events = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut evts = vec![];
        while let Some(e) = rx.recv().await {
            let is_terminal = matches!(e, AgentEvent::Done | AgentEvent::Error(_));
            evts.push(e);
            if is_terminal {
                break;
            }
        }
        evts
    })
    .await
    .expect("test timed out waiting for agent events");

    let _ = handle.await;

    println!("\n=== Events received ({}) ===", events.len());
    for (i, e) in events.iter().enumerate() {
        println!("  [{i:2}] {e:?}");
    }

    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::Done)),
        "Expected Done event, got: {:?}",
        events
            .iter()
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
    );

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::LlmResponse(_))),
        "Expected at least one LlmResponse event"
    );

    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Error(_))),
        "Unexpected Error event"
    );
}
