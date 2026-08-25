use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use data_catalog::{
    CatalogConfig, CatalogRuntime, build_package, catalog_mount_definitions, package::BuildOptions,
    publish_package, storage::operator_for_backend, validate_package,
};
use vfs::VfsManifest;

#[derive(Parser)]
#[command(
    name = "autonomics-catalog",
    version,
    about = "Build and publish versioned data packages."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a normalized package directory from a directory, archive, or file.
    Build {
        input: PathBuf,
        output: PathBuf,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long = "metadata", value_name = "KEY=VALUE")]
        metadata: Vec<String>,
        #[arg(long, value_name = "JSON")]
        payload: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Validate a normalized package directory.
    Validate { package: PathBuf },
    /// Publish a package and atomically advance the catalog generation.
    Publish {
        package: PathBuf,
        #[arg(long, default_value = "~/.autonomics/vfs.toml")]
        config: PathBuf,
    },
    /// List current catalog entries.
    List {
        #[arg(long, default_value = "~/.autonomics/vfs.toml")]
        config: PathBuf,
    },
    /// Show the VFS mounts that a catalog would generate.
    Mounts {
        #[arg(long, default_value = "~/.autonomics/vfs.toml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Build {
            input,
            output,
            id,
            version,
            kind,
            metadata,
            payload,
            force,
        } => {
            let payload = payload
                .map(|source| {
                    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&source)
                })
                .transpose()
                .map_err(|error| format!("invalid payload JSON: {error}"))?
                .unwrap_or_default();
            let metadata = parse_metadata(metadata)?;
            let built = build_package(
                input,
                output,
                BuildOptions {
                    id,
                    version,
                    kind,
                    metadata,
                    payload,
                    force,
                },
            )
            .map_err(|error| error.to_string())?;
            println!("{}", built.path.display());
            println!(
                "{}",
                built
                    .manifest
                    .digest
                    .expect("built manifests carry a digest")
            );
        }
        Command::Validate { package } => {
            let manifest = validate_package(package).map_err(|error| error.to_string())?;
            print_json(&manifest)?;
        }
        Command::Publish { package, config } => {
            let (manifest, catalog_config, operator) = load_backend(&config).await?;
            let entry = publish_package(&package, &catalog_config, &operator).await?;
            let _ = manifest;
            print_json(&entry)?;
        }
        Command::List { config } => {
            let (manifest, catalog_config, _) = load_backend(&config).await?;
            let runtime = CatalogRuntime::load(&manifest, &catalog_config).await?;
            let current: Vec<_> = runtime.index.current_entries().collect();
            print_json(&current)?;
        }
        Command::Mounts { config } => {
            let source = std::fs::read_to_string(expand_home(config)?)
                .map_err(|error| format!("read VFS config: {error}"))?;
            let manifest = VfsManifest::from_toml(&source).map_err(|error| error.to_string())?;
            let catalog_config = CatalogConfig::from_vfs_toml(&source)?;
            let runtime = CatalogRuntime::load(&manifest, &catalog_config).await?;
            let mounts = catalog_mount_definitions(&manifest, &runtime.index, &catalog_config)?;
            print_json(&mounts)?;
        }
    }
    Ok(())
}

async fn load_backend(
    config: &std::path::Path,
) -> Result<(VfsManifest, CatalogConfig, opendal::Operator), String> {
    let source = std::fs::read_to_string(expand_home(config)?)
        .map_err(|error| format!("read VFS config: {error}"))?;
    let manifest = VfsManifest::from_toml(&source).map_err(|error| error.to_string())?;
    let catalog_config = CatalogConfig::from_vfs_toml(&source)?;
    let operator = operator_for_backend(&manifest, &catalog_config.backend)?;
    Ok((manifest, catalog_config, operator))
}

fn parse_metadata(values: Vec<String>) -> Result<BTreeMap<String, String>, String> {
    let mut metadata = BTreeMap::new();
    for value in values {
        let (key, value) = value
            .split_once('=')
            .ok_or_else(|| format!("metadata must be KEY=VALUE, got `{value}`"))?;
        metadata.insert(key.to_string(), value.to_string());
    }
    Ok(metadata)
}

fn expand_home(path: impl AsRef<std::path::Path>) -> Result<std::path::PathBuf, String> {
    let path = path.as_ref().to_path_buf();
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        let home = std::env::var_os("HOME").ok_or_else(|| "$HOME is not set".to_string())?;
        return Ok(std::path::Path::new(&home).join(rest));
    }
    Ok(path)
}

fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    println!("{}", String::from_utf8_lossy(&bytes));
    Ok(())
}
