use agentik_kms::TursoKmsStorage;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let source = args
        .next()
        .unwrap_or_else(|| "~/.autonomics/agent.db".to_string());
    let destination = args
        .next()
        .unwrap_or_else(|| "~/.autonomics/knowledge.db".to_string());
    let source = shellexpand(&source);
    let destination = shellexpand(&destination);

    let store = match TursoKmsStorage::open(&destination).await {
        Ok(store) => store,
        Err(error) => {
            eprintln!("open destination {destination}: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = store.migrate_from_agent_db(&source).await {
        eprintln!("migrate {source} -> {destination}: {error}");
        std::process::exit(1);
    }
    println!("memory migration checked: {source} -> {destination}");
}

fn shellexpand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::Path::new(&home)
                .join(rest)
                .to_string_lossy()
                .into_owned();
        }
    }
    path.to_string()
}
