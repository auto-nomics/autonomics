use std::path::{Path, PathBuf};

use clap::Args;
use data_catalog::RemoteCatalog;
use data_catalog::error::Result;
use data_catalog::hf::resolve_hf_token;

use crate::common::{default_cache_root, load_remote, open_cache, print_json};

#[derive(Args)]
pub struct InstallArgs {
    /// Hugging Face dataset repository (`owner/name`) identifying the package
    /// to install. With the central registry this is resolved against
    /// `index.json`; without it, the repo is read directly.
    pub repo: String,
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
    /// Optional HF repo to limit the update to a single package; defaults to
    /// updating every declared repository.
    #[arg(long)]
    pub repo_filter: Option<String>,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    #[arg(long, default_value_t = default_cache_root())]
    pub cache: String,
}

pub async fn run_install(args: InstallArgs) -> Result<()> {
    // Resolve from the central registry when the configured index exists;
    // otherwise read the package repo directly. Both branches land in the
    // same cache directory.
    let remote = RemoteCatalog::hf(
        &args.repo,
        args.revision.clone(),
        resolve_hf_token(args.token.clone()),
    )?;
    let catalog = open_cache(Path::new(&args.cache))?;
    let entry = catalog
        .install(
            &remote,
            &args.repo,
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
    let updated = catalog.update(&remote, args.repo_filter.as_deref()).await?;
    print_json(&updated)
}
