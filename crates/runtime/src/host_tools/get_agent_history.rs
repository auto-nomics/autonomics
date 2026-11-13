use agentik_core::tools::ToolFunction;
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use super::visible_from;
use crate::control::HostControl;

#[tool(
    name = "get_agent_history",
    description = "Read the persisted conversation history of a visible agent \
                   (yourself, your direct parent, or one of your direct children). \
                   Returns bounded message summaries, not raw history: thinking, images \
                   and tool-result bodies are omitted; text and tool inputs may be truncated. \
                   Use offset to page older messages and inspect truncation flags. Read actual \
                   task artifacts with their own tools instead of recursively dumping history."
)]
pub(super) struct GetAgentHistoryInput {
    /// Full or short name of the target agent.
    pub(super) agent_name: String,
    /// Maximum number of messages to return. Default 20; clamped to 100.
    #[default = 20]
    limit: Option<usize>,
    /// Number of newest messages to skip for older pages; maximum 10000.
    offset: Option<usize>,
}

pub(super) struct GetAgentHistoryTool {
    pub(super) control: HostControl,
    pub(super) self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for GetAgentHistoryTool {
    type Input = GetAgentHistoryInput;

    async fn run(
        &self,
        input: GetAgentHistoryInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let limit = input.limit.unwrap_or(20).clamp(1, 100);
        let offset = input.offset.unwrap_or(0);
        if offset > 10000 {
            return Err(agentik_core::tools::ToolError::ValidationFailed {
                message: "history offset must be at most 10000".into(),
            });
        }
        match self
            .control
            .agent_history(&input.agent_name, offset + limit + 1)
            .await
        {
            Some(history) => {
                // Visibility gate on the resolved target path. A history
                // whose path cannot be parsed is treated as invisible.
                let visible = agentik_types::AgentPath::try_from(history.agent_path.as_str())
                    .is_ok_and(|p| visible_from(&self.self_path, &p));
                if !visible {
                    return Ok(ToolResult::success(format!(
                        "Agent '{}' not found or not visible to you.",
                        input.agent_name
                    )));
                }
                Ok(ToolResult::success_json(history_summary(
                    serde_json::to_value(history).unwrap_or_default(),
                    offset,
                    limit,
                )))
            }
            None => Ok(ToolResult::success(
                "Failed to read agent history — host unavailable.",
            )),
        }
    }
}

fn excerpt(value: &str, max: usize) -> String {
    let end = value
        .char_indices()
        .map(|(i, _)| i)
        .find(|i| *i >= max)
        .unwrap_or(value.len());
    value[..end].to_owned()
}
fn history_summary(history: serde_json::Value, offset: usize, limit: usize) -> serde_json::Value {
    use serde_json::json;
    let source = history["messages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let end = source.len().saturating_sub(offset);
    let start = end.saturating_sub(limit);
    let mut messages=source[start..end].iter().map(|message|{
        let mut omitted=0;
        let content=message["content"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let blocks=content.iter().filter_map(|block|{
            match block["type"].as_str(){
                Some("thinking"|"redacted_thinking"|"image")=>{omitted+=1;None},
                Some("text")=>{let text=block["text"].as_str().unwrap_or("");let safe=if text.contains("data:image/"){"[image-bearing text omitted]"}else{text};let preview=excerpt(safe,2000);Some(json!({"type":"text","text":preview,"truncated":safe!=text||preview.len()<text.len()}))},
                Some("tool_use")=>{let input=block["input"].to_string();let safe=if input.contains("data:image/")||input.contains("\"thinking\"") || input.contains("\"data\"") || input.contains("\"signature\"") {"[image or thinking-bearing input omitted]"}else{&input};Some(json!({"type":"tool_use","id":block["id"].as_str().map(|v|excerpt(v,512)),"name":block["name"].as_str().map(|v|excerpt(v,512)),"input_preview":excerpt(safe,2000),"input_truncated":safe!=input||safe.len()>2000}))},
                Some("tool_result")=>Some(json!({"type":"tool_result","tool_use_id":block["tool_use_id"].as_str().map(|v|excerpt(v,512)),"is_error":block["is_error"],"content_omitted":true})),
                _=>{omitted+=1;None}
            }
        }).take(8).collect::<Vec<_>>();
        json!({"id":message["id"].as_str().map(|v|excerpt(v,512)),"role":message["role"].as_str().map(|v|excerpt(v,512)),"stop_reason":message["stop_reason"].as_str().map(|v|excerpt(v,512)),"content":blocks,"content_incomplete":omitted>0||content.len()>8})
    }).collect::<Vec<_>>();
    let initially_selected = messages.len();
    loop {
        let count = messages.len();
        let has_more = source.len() > offset + count;
        let result = json!({"agent_path":history["agent_path"],"agent_id":history["agent_id"],"session_id":history["session_id"],"history_scope":"bounded_message_summaries","offset":offset,"returned":count,"messages":messages,"has_more_older":has_more,"next_offset":if has_more{Some(offset+count)}else{None},"message_budget_truncated":count<initially_selected,"limitations":"Thinking, images and tool-result bodies omitted; text/tool input previews may be truncated. Read task artifacts with their own tools."});
        if result.to_string().len() <= 64000 || messages.is_empty() {
            return result;
        }
        messages.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::history_summary;
    use serde_json::json;
    #[test]
    fn summaries_omit_images_thinking_and_recursive_results_with_stable_pagination() {
        let messages=(0..6).map(|i|json!({"id":format!("m{i}"),"role":"assistant","content":[{"type":"thinking","thinking":"PRIVATE-THINKING"},{"type":"image","source":{"data":"PRIVATE-IMAGE"}},{"type":"text","text":format!("public {i}")},{"type":"tool_result","content":"PRIVATE-HUGE-RECURSIVE-HISTORY","is_error":false,"tool_use_id":"t"}]})).collect::<Vec<_>>();
        let history =
            json!({"agent_path":"/root/a","agent_id":"a","session_id":"s","messages":messages});
        let first = history_summary(history.clone(), 0, 2);
        assert_eq!(first["returned"], 2);
        assert_eq!(first["next_offset"], 2);
        assert_eq!(first["messages"][0]["id"], "m4");
        assert_eq!(first["messages"][1]["id"], "m5");
        let next = history_summary(history, 2, 2);
        assert_eq!(next["messages"][0]["id"], "m2");
        assert_eq!(next["messages"][1]["id"], "m3");
        let encoded = first.to_string();
        assert!(!encoded.contains("PRIVATE-"));
        assert!(encoded.contains("content_omitted"));
        assert!(encoded.contains("public 5"));
    }
    #[test]
    fn summaries_bound_unicode_tool_inputs_blocks_and_total_bytes() {
        let input = "中文".repeat(30000);
        let message = json!({"id":"m","role":"assistant","content":[{"type":"text","text":input},{"type":"tool_use","id":"t","name":"draw","input":{"value":input}},{"type":"text","text":"data:image/png;base64,PRIVATE-IMAGE"}]});
        let history = json!({"agent_path":"/root/a","messages":vec![message;100]});
        let summary = history_summary(history, 0, 100);
        assert!(summary.to_string().len() <= 64000);
        assert_eq!(summary["message_budget_truncated"], true);
        assert!(summary["returned"].as_u64().unwrap() > 0);
        assert!(summary["next_offset"].as_u64().unwrap() > 0);
        assert!(!summary.to_string().contains("PRIVATE-IMAGE"));
    }
    #[test]
    fn optional_saved_history_is_bounded_without_replaying_private_blocks() {
        let Ok(source)=std::env::var("AGENT_HISTORY_SUMMARY_SOURCE") else{return;};
        let messages:serde_json::Value=serde_json::from_slice(&std::fs::read(source).unwrap()).unwrap();
        let summary=history_summary(json!({"agent_path":"/root/figure","messages":messages}),0,40);
        let encoded=serde_json::to_string_pretty(&summary).unwrap();
        assert!(summary.to_string().len()<=64000);
        assert!(!encoded.contains("\"type\": \"thinking\""));
        assert!(!encoded.contains("data:image/"));
        if let Ok(output)=std::env::var("AGENT_HISTORY_SUMMARY_OUTPUT"){std::fs::write(output,encoded).unwrap();}
    }

}
