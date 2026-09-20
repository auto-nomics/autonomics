use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use data_catalog::error::Result;
use data_catalog::{
    CatalogConfig, LocalCatalog, RemoteCatalog, build_package, package::BuildOptions,
    publish_package, storage::operator_for_backend, validate_package,
};
use vfs::VfsManifest;

#[derive(Parser)]
#[command(
    name = "autonomics-catalog",
    version,
    about = "Build, publish, install, and inspect versioned data packages."
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
    /// Publish a package and atomically advance the remote catalog generation.
    Publish {
        package: PathBuf,
        #[arg(long, default_value = "~/.autonomics/vfs.toml")]
        config: PathBuf,
    },
    /// Search current entries in the remote catalog.
    Search {
        query: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value = "~/.autonomics/vfs.toml")]
        config: PathBuf,
    },
    /// Download and verify one package into the local cache.
    Install {
        id: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        digest: Option<String>,
        #[arg(long, default_value = "~/.autonomics/vfs.toml")]
        config: PathBuf,
        #[arg(long, default_value = "~/.autonomics/catalog")]
        cache: PathBuf,
    },
    /// Install remote current versions missing from the local cache.
    Update {
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value = "~/.autonomics/vfs.toml")]
        config: PathBuf,
        #[arg(long, default_value = "~/.autonomics/catalog")]
        cache: PathBuf,
    },
    /// List current entries installed in the local cache.
    List {
        #[arg(long, default_value = "~/.autonomics/catalog")]
        cache: PathBuf,
    },
    /// Show the VFS mounts generated from the local cache.
    Mounts {
        #[arg(long, default_value = "~/.autonomics/catalog")]
        cache: PathBuf,
        #[arg(long, default_value = "catalog-cache")]
        backend: String,
    },
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
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
            let (_, catalog_config, operator) = load_backend(&config).await?;
            let entry = publish_package(&package, &catalog_config, &operator).await?;
            print_json(&entry)?;
        }
        Command::Search {
            query,
            kind,
            limit,
            config,
        } => {
            let remote = load_remote(&config).await?;
            let index = remote.index().await?;
            let entries = index.search(query.as_deref().unwrap_or(""), kind.as_deref(), limit);
            print_json(&entries)?;
        }
        Command::Install {
            id,
            version,
            digest,
            config,
            cache,
        } => {
            let remote = load_remote(&config).await?;
            let catalog = LocalCatalog::open(expand_home(cache)?)?;
            let entry = catalog
                .install(&remote, &id, version.as_deref(), digest.as_deref())
                .await?;
            print_json(&entry)?;
        }
        Command::Update { id, config, cache } => {
            let remote = load_remote(&config).await?;
            let catalog = LocalCatalog::open(expand_home(cache)?)?;
            let updated = catalog.update(&remote, id.as_deref()).await?;
            print_json(&updated)?;
        }
        Command::List { cache } => {
            let catalog = LocalCatalog::open(expand_home(cache)?)?;
            let index = catalog.index()?;
            let current: Vec<_> = index.current_entries().collect();
            print_json(&current)?;
        }
        Command::Mounts { cache, backend } => {
            let catalog = LocalCatalog::open(expand_home(cache)?)?;
            let mounts = catalog.mount_definitions(&backend, true)?;
            print_json(&mounts)?;
        }
    }
    Ok(())
}

async fn load_remote(config: &std::path::Path) -> Result<RemoteCatalog> {
    let source = std::fs::read_to_string(expand_home(config)?)
        .map_err(|error| format!("read VFS config: {error}"))?;
    let manifest = VfsManifest::from_toml(&source).map_err(|error| error.to_string())?;
    let catalog_config = CatalogConfig::from_vfs_toml(&source)?;
    RemoteCatalog::new(&manifest, &catalog_config)
}

async fn load_backend(
    config: &std::path::Path,
) -> Result<(VfsManifest, CatalogConfig, opendal::Operator)> {
    let source = std::fs::read_to_string(expand_home(config)?)
        .map_err(|error| format!("read VFS config: {error}"))?;
    let manifest = VfsManifest::from_toml(&source).map_err(|error| error.to_string())?;
    let catalog_config = CatalogConfig::from_vfs_toml(&source)?;
    let operator = operator_for_backend(&manifest, &catalog_config.backend)?;
    Ok((manifest, catalog_config, operator))
}

fn parse_metadata(values: Vec<String>) -> Result<BTreeMap<String, String>> {
    let mut metadata = BTreeMap::new();
    for value in values {
        let (key, value) = value
            .split_once('=')
            .ok_or_else(|| format!("metadata must be KEY=VALUE, got `{value}`"))?;
        metadata.insert(key.to_string(), value.to_string());
    }
    Ok(metadata)
}

fn expand_home(path: impl AsRef<std::path::Path>) -> Result<std::path::PathBuf> {
    let path = path.as_ref().to_path_buf();
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        let home = std::env::var_os("HOME").ok_or_else(|| "$HOME is not set".to_string())?;
        return Ok(std::path::Path::new(&home).join(rest));
    }
    Ok(path)
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    println!("{}", String::from_utf8_lossy(&bytes));
    Ok(())
}
