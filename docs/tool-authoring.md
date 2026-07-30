# Tool Authoring

[English](tool-authoring.md) | [中文](tool-authoring_zh.md)

Tools implement `ToolFunction` with an associated strongly-typed `Input`. The framework deserializes the LLM's JSON into `Input` before `run` is called, so tool bodies stay free of `serde_json::Value` plumbing.

```rust
#[derive(Deserialize, agentik_proc::ToolInput)]
#[tool(name = "get_weather", description = "Current weather for a city")]
struct WeatherInput {
    #[desc = "City name"]
    city: String,

    #[desc = "Units: metric or imperial"]
    #[default = "metric"]
    units: Option<String>,
}

struct WeatherTool;

#[async_trait]
impl ToolFunction for WeatherTool {
    type Input = WeatherInput;
    async fn run(&self, i: WeatherInput) -> Result<ToolResult, ToolError> {
        // ... fetch weather ...
        Ok(ToolResult::success("weather", format!("{}: sunny", i.city)))
    }
}
```

The built-in lifecycle tools (`attempt_complete`, `abort_task`) drive agent state transitions (`IDLE` / `ABORTED`).

For more on the agent runtime and proc macros, see [Agent Runtime](agent-runtime.md).
