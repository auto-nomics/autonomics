use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::package::BuildOptions;
use data_catalog::{build_package, validate_package};

use crate::common::{parse_metadata, print_json};

#[derive(Args)]
pub struct BuildArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    /// Hugging Face repo in `owner/name` form. Overrides the `repo` field in
    /// `package.json` if both are set.
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long = "metadata", value_name = "KEY=VALUE")]
    pub metadata: Vec<String>,
    #[arg(long, value_name = "JSON")]
    pub payload: Option<String>,
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct ValidateArgs {
    pub package: PathBuf,
}

pub fn run_build(args: BuildArgs) -> Result<()> {
    let payload = args
        .payload
        .map(|source| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&source))
        .transpose()
        .map_err(|error| format!("invalid payload JSON: {error}"))?
        .unwrap_or_default();
    let metadata = parse_metadata(&args.metadata)?;
    let built = build_package(
        args.input,
        args.output,
        BuildOptions {
            repo: args.repo,
            version: args.version,
            kind: args.kind,
            metadata,
            payload,
            force: args.force,
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
    Ok(())
}

pub fn run_validate(args: ValidateArgs) -> Result<()> {
    let manifest = validate_package(args.package).map_err(|error| error.to_string())?;
    print_json(&manifest)
}
