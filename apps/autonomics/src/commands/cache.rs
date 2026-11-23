//! OpenGWAS cache management commands.

use std::path::PathBuf;

use i18n::t;

use opengwas::OpengwasClient;

use crate::cli::{ClearOpengwasArgs, RefreshOpengwasArgs};

/// Resolve the OpenGWAS token from env. Mirrors the convention used by
/// the `runtime` crate and `OpengwasClient::new`.
fn opengwas_token() -> color_eyre::Result<String> {
    std::env::var("OPENGWAS_TOKEN")
        .map_err(|_| color_eyre::eyre::eyre!("{}", t!("cli.cache.tokenNotSet")))
}

pub fn run_refresh_opengwas(args: RefreshOpengwasArgs) -> color_eyre::Result<()> {
    let token = opengwas_token()?;
    let client = OpengwasClient::with_cache_dir(Some(&token), default_opengwas_cache_dir())?;

    if args.show_cache_path {
        println!(
            "{}",
            t!("cli.cache.cacheFile", path = client.cache_file_path().display())
        );
    }

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;

    let refreshed = runtime.block_on(async { client.refresh_disk_cache().await })?;
    println!(
        "{}",
        t!(
            "cli.cache.refreshed",
            count = refreshed.len(),
            path = client.cache_file_path().display()
        )
    );
    Ok(())
}

pub fn run_clear_opengwas(args: ClearOpengwasArgs) -> color_eyre::Result<()> {
    let token = opengwas_token()?;
    let client = OpengwasClient::with_cache_dir(Some(&token), default_opengwas_cache_dir())?;
    let path = client.cache_file_path();

    if !args.yes {
        eprint!("{}", t!("cli.cache.confirmDelete", path = path.display()));
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("{}", t!("cli.cache.aborted"));
            return Ok(());
        }
    }

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async { client.clear_disk_cache().await })?;
    println!("{}", t!("cli.cache.deleted", path = path.display()));
    Ok(())
}

/// Resolve the cache directory the same way `OpengwasClient::new` does,
/// without round-tripping through env twice.
fn default_opengwas_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OPENGWAS_CACHE_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("opengwas");
    }
    std::env::temp_dir().join("opengwas")
}
