use std::path::{Path, PathBuf};

use clap::Args;
use data_catalog::RemoteCatalog;
use data_catalog::error::Result;

use crate::common::{default_cache_root, load_remote, open_cache, print_json, resolve_hf_token};

#[derive(Args)]
pub struct InstallArgs {
    pub id: String,
    /// Hugging Face dataset repository (`owner/name`); read it instead of vfs.toml.
    #[arg(long, value_name = "REPO")]
    pub repo: Option<String>,
    /// Hugging Face branch; defaults to the repository main branch.
    #[arg(long)]
    pub revision: Option<String>,
    /// Hugging Face token; defaults to $HUGGING_FACE_TOKEN, then $HF_TOKEN.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub digest: Option<String>,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    #[arg(long, default_value_t = default_cache_root())]
    pub cache: String,
}

#[derive(Args)]
pub struct UpdateArgs {
    /// Hugging Face dataset repository (`owner/name`); read it instead of vfs.toml.
    #[arg(long, value_name = "REPO")]
    pub repo: Option<String>,
    /// Hugging Face branch; defaults to the repository main branch.
    #[arg(long)]
    pub revision: Option<String>,
    /// Hugging Face token; defaults to $HUGGING_FACE_TOKEN, then $HF_TOKEN.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
    #[arg(long)]
    pub id: Option<String>,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    #[arg(long, default_value_t = default_cache_root())]
    pub cache: String,
}

pub async fn run_install(args: InstallArgs) -> Result<()> {
    let remote = match args.repo {
        Some(repo_id) => RemoteCatalog::hf(
            &repo_id,
            args.revision.clone(),
            resolve_hf_token(args.token.clone()),
        )?,
        None => load_remote(&args.config).await?,
    };
    let catalog = open_cache(Path::new(&args.cache))?;
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
    let remote = match args.repo {
        Some(repo_id) => RemoteCatalog::hf(
            &repo_id,
            args.revision.clone(),
            resolve_hf_token(args.token.clone()),
        )?,
        None => load_remote(&args.config).await?,
    };
    let catalog = open_cache(Path::new(&args.cache))?;
    let updated = catalog.update(&remote, args.id.as_deref()).await?;
    print_json(&updated)
}
