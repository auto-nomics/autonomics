use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;

use crate::common::{load_remote, open_cache, print_json};

#[derive(Args)]
pub struct InstallArgs {
    pub id: String,
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub digest: Option<String>,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    #[arg(long, default_value = "~/.autonomics/catalog")]
    pub cache: PathBuf,
}

#[derive(Args)]
pub struct UpdateArgs {
    #[arg(long)]
    pub id: Option<String>,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    #[arg(long, default_value = "~/.autonomics/catalog")]
    pub cache: PathBuf,
}

pub async fn run_install(args: InstallArgs) -> Result<()> {
    let remote = load_remote(&args.config).await?;
    let catalog = open_cache(&args.cache)?;
    let entry = catalog
        .install(
            &remote,
            &args.id,
            args.version.as_deref(),
            args.digest.as_deref(),
        )
        .await?;
    print_json(&entry)
}

pub async fn run_update(args: UpdateArgs) -> Result<()> {
    let remote = load_remote(&args.config).await?;
    let catalog = open_cache(&args.cache)?;
    let updated = catalog.update(&remote, args.id.as_deref()).await?;
    print_json(&updated)
}
