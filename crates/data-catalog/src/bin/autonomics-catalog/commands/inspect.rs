use std::path::Path;

use clap::Args;
use data_catalog::error::Result;

use crate::common::{default_cache_root, open_cache, print_json};

#[derive(Args)]
pub struct ListArgs {
    #[arg(long, default_value_t = default_cache_root())]
    pub cache: String,
}

#[derive(Args)]
pub struct MountsArgs {
    #[arg(long, default_value_t = default_cache_root())]
    pub cache: String,
    #[arg(long, default_value = "catalog-cache")]
    pub backend: String,
}

pub fn run_list(args: ListArgs) -> Result<()> {
    let catalog = open_cache(Path::new(&args.cache))?;
    let index = catalog.index()?;
    let current: Vec<_> = index.current_entries().collect();
    print_json(&current)
}

pub fn run_mounts(args: MountsArgs) -> Result<()> {
    let catalog = open_cache(Path::new(&args.cache))?;
    let mounts = catalog.mount_definitions(&args.backend, true)?;
    print_json(&mounts)
}
