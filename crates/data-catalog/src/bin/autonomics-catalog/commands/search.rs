use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;

use crate::common::{load_remote, print_json};

#[derive(Args)]
pub struct SearchArgs {
    pub query: Option<String>,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
}

pub async fn run(args: SearchArgs) -> Result<()> {
    let remote = load_remote(&args.config).await?;
    let index = remote.index().await?;
    let entries = index.search(
        args.query.as_deref().unwrap_or(""),
        args.kind.as_deref(),
        args.limit,
    );
    print_json(&entries)
}
