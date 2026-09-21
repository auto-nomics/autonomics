use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::hf::{
    HfPublishTarget, package_repo_prefix_for_index, publish_package_to_hf, resolve_hf_token,
};

use crate::common::{load_catalog_config, print_json};

#[derive(Args)]
pub struct PublishArgs {
    pub package: PathBuf,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    /// Hugging Face registry repository (`owner/name`).
    #[arg(long, value_name = "REPO")]
    pub repo: Option<String>,
    /// Publish to this exact package repository (`owner/name`).
    #[arg(long, value_name = "REPO")]
    pub package_repo: Option<String>,
    /// Prefix for per-package Hugging Face repositories (`owner/name`).
    #[arg(long, value_name = "PREFIX")]
    pub package_prefix: Option<String>,
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

    let package_repo_prefix = match args.package_prefix.as_deref() {
        Some(prefix) => prefix.to_string(),
        None => {
            let configured = load_catalog_config(&args.config)
                .ok()
                .and_then(|config| config.repository_prefix);
            match configured {
                Some(prefix) => prefix,
                None => package_repo_prefix_for_index(&index_repo_id)?,
            }
        }
    };

    Ok(HfPublishTarget {
        index_repo_id,
        package_repo: args.package_repo.clone(),
        package_repo_prefix,
        revision,
        token: resolve_hf_token(args.token.clone()),
        create_repository: args.create_repo,
    })
}
