//! Opt-in configured model over isolated RuntimeHost + production HTTP/login/SSE.
use gateway::testing::start_persistent_gateway_with_model;
use rusqlite::{Connection, OpenFlags};
use std::{io::Write, path::PathBuf};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        return Err(
            "usage: visualization_gateway_live fresh-directory config.db provider model".into(),
        );
    }
    let directory = PathBuf::from(&args[0]);
    if !directory.is_dir() || std::fs::read_dir(&directory)?.next().is_some() {
        return Err("use a fresh existing output directory".into());
    }
    let connection = Connection::open_with_flags(&args[1], OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let model = runtime::model_bootstrap::resolve_model_spec(
        &connection,
        &format!("{}:{}", args[2], args[3]),
    )
    .ok_or("configured model unavailable")?;
    if !model.model_info.vision_ability
        || !model.model_info.supports_function_calling
        || !model.model_info.supports_streaming
    {
        return Err("configured model requires vision, tools and streaming".into());
    }
    // Match the configured production model; test-only caps can truncate reasoning.
    let max_output_tokens = model.model_info.max_output_tokens;
    drop(connection);
    let runtime_directory = directory.join("runtime");
    let server = start_persistent_gateway_with_model(model, runtime_directory.clone()).await?;
    let agent = server
        .client()
        .spawn_agent("figure", "/root", "researcher", None, None)
        .await?;
    let ready = serde_json::to_vec(
        &serde_json::json!({"origin":format!("http://{}",server.addr),"token":server.token,"agent":agent,"max_output_tokens":max_output_tokens,"runtime_directory":runtime_directory}),
    )?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(directory.join("ready.json"))?
        .write_all(&ready)?;
    while !directory.join("stop").exists() {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    server.stop().await;
    Ok(())
}
