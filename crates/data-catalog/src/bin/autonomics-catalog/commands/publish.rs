use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::hf::{HfPublishTarget, publish_package_to_hf};
use data_catalog::publish_package;

use crate::common::{load_backend, print_json};

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
        let token = args.token.or_else(|| {
            std::env::var("HUGGING_FACE_TOKEN")
                .ok()
                .filter(|token| !token.is_empty())
        });
        publish_package_to_hf(
            &args.package,
            &HfPublishTarget {
                repo_id,
                revision: args.revision,
                token,
                create_repository: args.create_repo,
            },
        )
        .await?
    } else {
        let (_, catalog_config, operator) = load_backend(&args.config).await?;
        publish_package(&args.package, &catalog_config, &operator).await?
    };
    print_json(&entry)
}
