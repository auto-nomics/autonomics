use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::hf::{
    HfPublishTarget, package_repo_prefix_for_index, publish_package_to_hf, resolve_hf_token,
};
use data_catalog::publish_package;
use data_catalog::storage::operator_for_backend;

use crate::common::{load_catalog_config, print_json};

#[derive(Args)]
pub struct PublishArgs {
    pub package: PathBuf,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
    /// Publish to this Hugging Face dataset repository (`owner/name`).
    #[arg(long, value_name = "REPO")]
    pub repo: Option<String>,
    /// Publish to this exact package repository (`owner/name`).
    #[arg(long, value_name = "REPO")]
    pub package_repo: Option<String>,
    /// Prefix for per-package Hugging Face dataset repositories (`owner/name`).
    /// Defaults to `repository_prefix` from vfs.toml, or a prefix derived from REPO.
    #[arg(long, value_name = "PREFIX")]
    pub package_prefix: Option<String>,
    /// Hugging Face branch; defaults to the repository main branch.
    #[arg(long)]
    pub revision: Option<String>,
    /// Create the Hugging Face repository when it does not exist yet.
    #[arg(long)]
    pub create_repo: bool,
    /// Hugging Face token; defaults to $HUGGING_FACE_TOKEN, then $HF_TOKEN.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
}

pub async fn run(args: PublishArgs) -> Result<()> {
    if let Some(target) = resolve_hf_target(&args)? {
        let entry = publish_package_to_hf(&args.package, &target).await?;
        print_json(&entry)?;
        return Ok(());
    }

    let (manifest, catalog_config) = load_catalog_config(&args.config)?;
    let backend = catalog_config.backend.clone().unwrap_or_default();
    let operator = operator_for_backend(&manifest, &backend)?;
    let entry = publish_package(&args.package, &catalog_config, &operator).await?;
    print_json(&entry)
}

fn resolve_hf_target(args: &PublishArgs) -> Result<Option<HfPublishTarget>> {
    if let Some(index_repo_id) = args.repo.as_deref() {
        let package_repo_prefix = match args.package_prefix.as_deref() {
            Some(prefix) => prefix.to_string(),
            None => package_repo_prefix_for_index(index_repo_id)?,
        };
        return Ok(Some(HfPublishTarget {
            index_repo_id: index_repo_id.to_string(),
            package_repo: args.package_repo.clone(),
            package_repo_prefix,
            revision: args.revision.clone(),
            token: resolve_hf_token(args.token.clone()),
            create_repository: args.create_repo,
        }));
    }

    if args.package_prefix.is_some() || args.package_repo.is_some() {
        return Err(
            "--package-prefix/--package-repo require --repo or a configured HF repository".into(),
        );
    }
    let (_, catalog_config) = load_catalog_config(&args.config)?;
    let Some(index_repo_id) = catalog_config.repository.clone() else {
        return Ok(None);
    };
    let package_repo_prefix = catalog_config
        .repository_prefix
        .clone()
        .map(Ok)
        .unwrap_or_else(|| package_repo_prefix_for_index(&index_repo_id))?;
    Ok(Some(HfPublishTarget {
        index_repo_id,
        package_repo: args.package_repo.clone(),
        package_repo_prefix,
        revision: catalog_config.revision.clone(),
        token: resolve_hf_token(args.token.clone()),
        create_repository: args.create_repo,
    }))
}
