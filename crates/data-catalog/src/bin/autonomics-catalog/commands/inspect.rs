use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;

use crate::common::{open_cache, print_json};

#[derive(Args)]
pub struct ListArgs {
    #[arg(long, default_value = "~/.autonomics/catalog")]
    pub cache: PathBuf,
}

#[derive(Args)]
pub struct MountsArgs {
    #[arg(long, default_value = "~/.autonomics/catalog")]
    pub cache: PathBuf,
    #[arg(long, default_value = "catalog-cache")]
    pub backend: String,
}

pub fn run_list(args: ListArgs) -> Result<()> {
    let catalog = open_cache(&args.cache)?;
    let index = catalog.index()?;
    let current: Vec<_> = index.current_entries().collect();
    print_json(&current)
}

pub fn run_mounts(args: MountsArgs) -> Result<()> {
    let catalog = open_cache(&args.cache)?;
    let mounts = catalog.mount_definitions(&args.backend, true)?;
    print_json(&mounts)
}
