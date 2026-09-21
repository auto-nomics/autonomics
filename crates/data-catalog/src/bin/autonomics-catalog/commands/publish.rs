use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::hf::{HfPublishTarget, publish_package_to_hf, resolve_hf_token};
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
    let entry = if let Some(repo_id) = args.repo {
        publish_package_to_hf(
            &args.package,
            &HfPublishTarget {
                repo_id,
                revision: args.revision,
                token: resolve_hf_token(args.token.clone()),
                create_repository: args.create_repo,
            },
        )
        .await?
    } else {
        let (manifest, catalog_config) = load_catalog_config(&args.config)?;
        if let Some(repository) = catalog_config.repository.clone() {
            publish_package_to_hf(
                &args.package,
                &HfPublishTarget {
                    repo_id: repository,
                    revision: catalog_config.revision.clone(),
                    token: resolve_hf_token(None),
                    create_repository: args.create_repo,
                },
            )
            .await?
        } else {
            let backend = catalog_config.backend.clone().unwrap_or_default();
            let operator = operator_for_backend(&manifest, &backend)?;
            publish_package(&args.package, &catalog_config, &operator).await?
        }
    };
    print_json(&entry)
}
