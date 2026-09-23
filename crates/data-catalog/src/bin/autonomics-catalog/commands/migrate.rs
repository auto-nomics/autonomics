use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::hf::{migrate_package_repository, migrate_registry_repository, resolve_hf_token};
use hf_hub::HFClientBuilder;

use crate::common::{expand_home, load_catalog_config, print_json};

#[derive(Args)]
pub struct MigrateArgs {
    /// Migrate only this package repository (`owner/name`). Default: migrate
    /// the registry from vfs.toml plus every package repository it
    /// references.
    #[arg(long, value_name = "REPO")]
    pub repo: Option<String>,
    /// Hugging Face branch; defaults to the repository main branch.
    #[arg(long)]
    pub revision: Option<String>,
    /// Hugging Face token; defaults to $HUGGING_FACE_TOKEN, then $HF_TOKEN.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
    /// Compute and print the migration plan without committing anything.
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long, default_value = "~/.autonomics/vfs.toml")]
    pub config: PathBuf,
}

pub async fn run(args: MigrateArgs) -> Result<()> {
    let mut builder = HFClientBuilder::new();
    if let Some(token) = resolve_hf_token(args.token.clone()) {
        builder = builder.token(token);
    }
    let client = builder
        .build()
        .map_err(|error| format!("build Hugging Face client: {error}"))?;
    let revision = args.revision.as_deref();

    match args.repo {
        Some(repo) => {
            let report = migrate_package_repository(&client, &repo, revision, args.dry_run).await?;
            print_json(&report)
        }
        None => {
            let config_path = expand_home(&args.config)?;
            let repository = load_catalog_config(&config_path)?
                .repository
                .ok_or_else(|| "catalog repository is required".to_string())?;
            let registry =
                migrate_registry_repository(&client, &repository, revision, args.dry_run).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&registry).unwrap_or_default()
            );
            // A fleet migration can run for hours; one failing repository
            // must not abandon the rest. Migration is idempotent, so a
            // failed repository is simply retried on the next run.
            let mut failures = Vec::new();
            for repo in &registry.repositories {
                match migrate_package_repository(&client, repo, revision, args.dry_run).await {
                    Ok(report) => print_json(&report)?,
                    Err(error) => {
                        eprintln!("error: migrating `{repo}` failed: {error}");
                        failures.push(repo.clone());
                    }
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "{} repository migration(s) failed: {}",
                    failures.len(),
                    failures.join(", ")
                )
                .into())
            }
        }
    }
}
