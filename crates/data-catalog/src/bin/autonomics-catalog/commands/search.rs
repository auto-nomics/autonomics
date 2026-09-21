use std::path::PathBuf;

use clap::Args;
use data_catalog::RemoteCatalog;
use data_catalog::error::Result;
use data_catalog::hf::resolve_hf_token;

use crate::common::{load_remote, print_json};

#[derive(Args)]
pub struct SearchArgs {
    pub query: Option<String>,
    /// Include historical versions, not only current entries.
    #[arg(long)]
    pub all: bool,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    /// Hugging Face dataset repository (`owner/name`); read it instead of vfs.toml.
    #[arg(long, value_name = "REPO")]
    pub repo: Option<String>,
    /// Hugging Face branch; defaults to the repository main branch.
    #[arg(long)]
    pub revision: Option<String>,
    /// Hugging Face token; defaults to $HUGGING_FACE_TOKEN, then $HF_TOKEN.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
}

pub async fn run(args: SearchArgs) -> Result<()> {
    let remote = match args.repo {
        Some(repo) => RemoteCatalog::hf(
            &repo,
            args.revision.clone(),
            resolve_hf_token(args.token.clone()),
        )?,
        None => load_remote(&args.config).await?,
    };
    let index = remote.index().await?;
    let query = args.query.as_deref().unwrap_or("");
    let kind = args.kind.as_deref();
    let entries = if args.all {
        index.search_all(query, kind, args.limit)
    } else {
        index.search(query, kind, args.limit)
    };
    print_json(&entries)
}
