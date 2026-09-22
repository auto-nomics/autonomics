use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::hf::{HfPublishTarget, publish_package_to_hf, resolve_hf_token};

use crate::common::{load_catalog_config, print_json};

#[derive(Args)]
pub struct PublishArgs {
    pub package: PathBuf,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    /// Hugging Face registry repository (`owner/name`).
    #[arg(long, value_name = "REPO")]
    pub repo: Option<String>,
    /// Hugging Face branch; defaults to the repository main branch.
    #[arg(long)]
    pub revision: Option<String>,
    /// Create missing Hugging Face repositories.
    #[arg(long)]
    pub create_repo: bool,
    /// Hugging Face token; defaults to $HUGGING_FACE_TOKEN, then $HF_TOKEN.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
}

pub async fn run(args: PublishArgs) -> Result<()> {
    let target = resolve_hf_target(&args)?;
    let entry = publish_package_to_hf(&args.package, &target).await?;
    print_json(&entry)
}

fn resolve_hf_target(args: &PublishArgs) -> Result<HfPublishTarget> {
    let (index_repo_id, revision) = if let Some(index_repo_id) = args.repo.as_deref() {
        (index_repo_id.to_string(), args.revision.clone())
    } else {
        let config = load_catalog_config(&args.config)?;
        let index_repo_id = config
            .repository
            .clone()
            .ok_or_else(|| "catalog repository is required".to_string())?;
        (index_repo_id, config.revision)
    };

    Ok(HfPublishTarget {
        index_repo_id,
        revision,
        token: resolve_hf_token(args.token.clone()),
        create_repository: args.create_repo,
    })
}
