use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::publish_package;

use crate::common::{load_backend, print_json};

#[derive(Args)]
pub struct PublishArgs {
    pub package: PathBuf,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
}

pub async fn run(args: PublishArgs) -> Result<()> {
    let (_, catalog_config, operator) = load_backend(&args.config).await?;
    let entry = publish_package(&args.package, &catalog_config, &operator).await?;
    print_json(&entry)
}
