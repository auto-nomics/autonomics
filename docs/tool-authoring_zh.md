# 工具编写

[English](tool-authoring.md) | [中文](tool-authoring_zh.md)

工具实现 `ToolFunction` 并关联一个强类型的 `Input`。框架在调用 `run` 之前将 LLM 的 JSON 反序列化为 `Input`，因此工具体无需处理 `serde_json::Value` 管道。

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
        // ... 获取天气 ...
        Ok(ToolResult::success("weather", format!("{}: sunny", i.city)))
    }
}
```

内置生命周期工具（`attempt_complete`、`abort_task`）驱动智能体状态转换（`IDLE` / `ABORTED`）。

更多关于智能体运行时和过程宏的细节，见 [智能体运行时](agent-runtime.md)。
