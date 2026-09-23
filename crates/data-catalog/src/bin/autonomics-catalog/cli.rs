use clap::{Parser, Subcommand};

use crate::commands::{build, init, inspect, install, migrate, publish, search};

#[derive(Parser)]
#[command(
    name = "autonomics-catalog",
    version,
    about = "Build, publish, install, and inspect versioned data packages."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Build a normalized package directory from a directory, archive, or file.
    Build(build::BuildArgs),
    /// Validate a normalized package directory.
    Validate(build::ValidateArgs),
    /// Publish a package and atomically advance the remote catalog generation.
    Publish(publish::PublishArgs),
    /// Search current entries in the remote catalog.
    Search(search::SearchArgs),
    /// Download and verify one package into the local cache.
    Install(install::InstallArgs),
    /// Install remote current versions missing from the local cache.
    Update(install::UpdateArgs),
    /// Rewrite legacy remote layouts (v2 indexes, v1 manifests) in place.
    Migrate(migrate::MigrateArgs),
    /// List current entries installed in the local cache.
    List(inspect::ListArgs),
    /// Show the VFS mounts generated from the local cache.
    Mounts(inspect::MountsArgs),
    /// Create new data package under current directory.
    Init(init::InitArgs),
}
