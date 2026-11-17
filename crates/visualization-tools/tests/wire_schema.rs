use agentik_sdk::types::messages::MessageCreateBuilder;
use agentik_sdk::wire::{WireProtocol, anthropic::AnthropicWire, openai::chat::OpenAiChatWire};
use serde_json::Value;

#[test]
fn wire_encoders_preserve_the_complete_registered_visualization_schema() {
    for definition in visualization_tools::registrations("unused.sqlite".into())
        .into_iter()
        .chain(visualization_tools::evidence_registrations())
        .chain(visualization_tools::bioicons_registrations(
            "unused.sqlite".into(),
        ))
        .filter(|tool| {
            matches!(
                tool.definition.name.as_str(),
                "visualization"
                    | "visualization_fit_svg_group_preview"
                    | "visualization_crop_image"
                    | "visualization_fit_svg_preview"
                    | "bioicons"
                    | "visualization_move_text"
                    | "visualization_add_text"
                    | "visualization_reflow_text"
                    | "visualization_find_space"
                    | "figure_evidence"
            )
        })
        .map(|tool| tool.definition)
    {
        let expected = serde_json::to_value(&definition.input_schema).unwrap();
        let mut params = MessageCreateBuilder::new("schema-audit", 100)
            .user("Audit only")
            .build();
        params.tools = Some(vec![definition]);
        for streaming in [false, true] {
            let anthropic = AnthropicWire.encode_request(&params, streaming).unwrap();
            let body: Value = serde_json::from_slice(&anthropic.body).unwrap();
            assert_eq!(body["tools"][0]["input_schema"], expected);
            let openai = OpenAiChatWire.encode_request(&params, streaming).unwrap();
            let body: Value = serde_json::from_slice(&openai.body).unwrap();
            assert_eq!(body["tools"][0]["function"]["parameters"], expected);
        }
    }
}
