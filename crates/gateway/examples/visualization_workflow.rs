//! Loopback-only full GUI/Gateway/RuntimeHost host with fixed model decisions.
//! No test HTTP shortcuts: the browser uses production login, chat, SSE and APIs.
use gateway::testing::{scripted_figure_model, start_mock_gateway_with_model};
use std::path::PathBuf;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("fresh fixture directory required")?,
    );
    if !directory.is_dir() || std::fs::read_dir(&directory)?.next().is_some() {
        return Err("use a fresh existing fixture directory".into());
    }
    let (model, trace) = scripted_figure_model();
    let gateway = start_mock_gateway_with_model(model).await;
    let agent = gateway
        .client()
        .spawn_agent("figure", "/root", "researcher", None, None)
        .await?;
    let ready = serde_json::to_vec(
        &serde_json::json!({"origin":format!("http://{}",gateway.addr),"token":gateway.token,"agent":agent}),
    )?;
    let ready_path = directory.join("ready.json");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(ready_path)?.write_all(&ready)?;
    loop {
        let snapshot = { serde_json::to_vec(&*trace.lock().unwrap())? };
        std::fs::write(directory.join("trace.tmp"), snapshot)?;
        std::fs::rename(directory.join("trace.tmp"), directory.join("trace.json"))?;
        if directory.join("stop").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    gateway.stop().await;
    Ok(())
}
