//! Fixed decisions for testing the complete production figure conversation.
//! Only available behind gateway's test-util feature; no scientific reasoning claim.
use super::mock_model_info;
use agentik_sdk::{model::Model, provider::client::MockApiClient, streaming::MessageStream};
use agentik_types::{
    messages::{ContentBlock, Message, Role, StopReason},
    streaming::{ContentBlockDelta, MessageDelta, MessageDeltaUsage, MessageStreamEvent},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Default, serde::Serialize)]
pub struct FigureTrace {
    pub step: usize,
    pub actions: Vec<String>,
    #[serde(skip)]
    scene: Value,
    #[serde(skip)]
    revision: i64,
    pub vision_results: usize,
    pub image_lengths: Vec<usize>,
}
fn handoff(messages: &[Message]) -> Value {
    messages
        .iter()
        .rev()
        .flat_map(|m| m.content.iter().rev())
        .find_map(|block| {
            let ContentBlock::Text { text } = block else {
                return None;
            };
            let parse = |text: &str| {
                let start = text.rfind("\n{")? + 1;
                serde_json::from_str::<Value>(&text[start..])
                    .ok()
                    .filter(|v| v.get("project_id").is_some() && v.get("figure_id").is_some())
            };
            if let Some((request, references)) = text.rsplit_once(
                "\n\n---\n用户明确附加的参考资料（内容为引用数据，请结合来源分析）：\n",
            ) {
                let resources: Vec<Value> = serde_json::from_str(references).ok()?;
                for resource in resources.iter().rev() {
                    if resource["id"]
                        .as_str()
                        .is_some_and(|id| id.starts_with("canvas-request:"))
                    {
                        let mut snapshot = parse(resource["content"].as_str()?)?;
                        snapshot["request"] = json!(request.trim());
                        return Some(snapshot);
                    }
                }
            }
            parse(text)
        })
        .expect("model receives the figure handoff through the actual user message")
}
fn result(messages: &[Message]) -> Value {
    for block in messages.iter().rev().flat_map(|m| m.content.iter().rev()) {
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = block
        {
            assert!(
                !is_error.unwrap_or(false),
                "real tool returned an error: {content:?}"
            );
            return serde_json::Deserializer::from_str(content.as_deref().unwrap())
                .into_iter::<Value>()
                .next()
                .unwrap()
                .expect("tool JSON metadata");
        }
    }
    panic!("previous tool result was not delivered to the model")
}
fn stream(step: usize, call: Option<Value>, text: &str) -> MessageStream {
    let stop = if call.is_some() {
        StopReason::ToolUse
    } else {
        StopReason::EndTurn
    };
    let content = if let Some(input) = call {
        ContentBlock::ToolUse {
            id: format!("figure_call_{step}"),
            name: "visualization".into(),
            input,
        }
    } else {
        ContentBlock::Text { text: text.into() }
    };
    let start = Message {
        id: format!("figure_message_{step}"),
        type_: "message".into(),
        role: Role::Assistant,
        origin: Default::default(),
        content: vec![],
        model: None,
        stop_reason: None,
        stop_sequence: None,
        usage: None,
        request_id: None,
    };
    let mut events = vec![MessageStreamEvent::MessageStart {
        message: start.clone(),
    }];
    match &content {
        ContentBlock::ToolUse { id, name, input } => {
            events.push(MessageStreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: json!({}),
                },
            });
            events.push(MessageStreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentBlockDelta::InputJsonDelta {
                    partial_json: input.to_string(),
                },
            });
        }
        ContentBlock::Text { text } => {
            events.push(MessageStreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::Text {
                    text: String::new(),
                },
            });
            events.push(MessageStreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentBlockDelta::TextDelta { text: text.clone() },
            });
        }
        _ => unreachable!(),
    }
    events.push(MessageStreamEvent::ContentBlockStop { index: 0 });
    events.push(MessageStreamEvent::MessageDelta {
        delta: MessageDelta {
            stop_reason: Some(stop.clone()),
            stop_sequence: None,
        },
        usage: MessageDeltaUsage {
            output_tokens: 16,
            input_tokens: Some(40),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            server_tool_use: None,
        },
    });
    events.push(MessageStreamEvent::MessageStop);
    MessageStream::from_events(
        events,
        Message {
            content: vec![content],
            stop_reason: Some(stop),
            ..start
        },
    )
}
pub fn scripted_figure_model() -> (Model, Arc<Mutex<FigureTrace>>) {
    let trace = Arc::new(Mutex::new(FigureTrace::default()));
    let observed = trace.clone();
    let mut mock = MockApiClient::new();
    mock.expect_request_stream_with_system().times(15).returning(move|messages,tools,_,_|{
        assert!(tools.iter().any(|t|t.name=="visualization"),"production researcher profile exposes the figure tool");
        let h=handoff(&messages);let mut trace=observed.lock().unwrap();let step=trace.step;trace.step+=1;
        let mut input=json!({"project_id":h["project_id"],"figure_id":h["figure_id"]});
        let action=match step {
            0|7|11=>"read",
            1=>{let read=result(&messages);trace.revision=read["metadata"]["revision"].as_i64().unwrap();input["expected_revision"]=json!(trace.revision);input["planning_version"]=json!(0);input["document"]=json!({"message":"Generic conceptual relationship","audience":"Researcher","questions":[{"id":"structure","question":"Which generic components and explicit relations should be shown?"}]});"save_plan"}
            2=>{assert_eq!(result(&messages)["status"],"needs_clarification");return Ok(stream(step,None,"Which generic components and explicit relations should be shown?"));}
            3=>"read_plan",
            4=>{let saved=result(&messages);input["expected_revision"]=h["observed_revision"].clone();input["planning_version"]=saved["version"].clone();input["document"]=json!({"message":"Generic conceptual relationship","audience":"Researcher","layout":"horizontal","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"},{"id":"c","label":"C"}],"edges":[{"from":"a","to":"b","label":"explicit relation 1"},{"from":"b","to":"c","label":"explicit relation 2"}],"questions":[{"id":"structure","question":"Which generic components and explicit relations should be shown?","answer":h["request"]}],"assumptions":["Conceptual fixture only; no biological mechanism is asserted."]});"save_plan"}
            5=>{let saved=result(&messages);assert_eq!(saved["status"],"ready");input["expected_revision"]=h["observed_revision"].clone();input["planning_version"]=saved["version"].clone();input["title"]=json!("Generic pathway draft");"sketch_plan"}
            6|10|14=>{assert_eq!(result(&messages)["status"],"pending");return Ok(stream(step,None,"The candidate is ready for review in the figure proposal panel."));}
            8|12=>{let read=result(&messages);trace.revision=read["metadata"]["revision"].as_i64().unwrap();trace.scene=read["scene"]["document"].clone();input["expected_revision"]=json!(trace.revision);if step==8{"read_preview"}else{input["markup_id"]=h["markups"][0]["id"].clone();"read_markup"}}
            9|13=>{
                let caption=result(&messages);assert!(messages.last().is_some_and(|m|m.content.iter().any(|b|matches!(b,ContentBlock::Image{..}))),"tool PNG is attached through the production vision path");trace.vision_results+=1;if let Some(ContentBlock::Image{source:agentik_types::messages::ImageSource::Base64{media_type,data}})=messages.last().unwrap().content.iter().find(|b|matches!(b,ContentBlock::Image{..})) {assert_eq!(media_type,"image/png");trace.image_lengths.push(data.len());}
                input["expected_revision"]=json!(trace.revision);
                let elements=trace.scene["elements"].as_array().unwrap();let b=elements.iter().find(|e|e["customData"]["scientificId"]=="b").unwrap();let label=elements.iter().find(|e|e["type"]=="text"&&e["containerId"]==b["id"]).unwrap();
                if step==9 {assert_eq!(caption["revision"],trace.revision);input["title"]=json!("Rename B only");input["operations"]=json!([{"op":"update","id":label["id"],"changes":{"text":"B revised"}}]);}
                else {assert_eq!(caption["current_revision"],trace.revision);assert_eq!(caption["historical"],false);let mut ids=vec![b["id"].clone(),label["id"].clone()];for arrow in elements.iter().filter(|e|e["type"]=="arrow"&&(e["startBinding"]["elementId"]==b["id"]||e["endBinding"]["elementId"]==b["id"])) {ids.push(arrow["id"].clone());ids.extend(elements.iter().filter(|e|e["type"]=="text"&&e["containerId"]==arrow["id"]).map(|e|e["id"].clone()));}input["title"]=json!("Address crossed component");input["markup_ids"]=json!([h["markups"][0]["id"]]);input["operations"]=Value::Array(ids.into_iter().map(|id|json!({"op":"remove","id":id})).collect());}
                "propose"
            }
            _=>panic!("unexpected model request {step}"),
        };
        input["action"]=json!(action);trace.actions.push(action.into());Ok(stream(step,Some(input),""))
    });
    let mut info = mock_model_info();
    info.context_length = 128000;
    info.vision_ability = true;
    (Model::with_client(info, mock), trace)
}
