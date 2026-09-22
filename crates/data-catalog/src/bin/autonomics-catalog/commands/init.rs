use std::path::PathBuf;

use clap::Args;
use data_catalog::error::Result;
use data_catalog::package::{
    BuildOptions, PACKAGE_SPEC, PackageSpec, build_package, validate_package,
};

use crate::common::parse_metadata;

#[derive(Args)]
pub struct InitArgs {
    /// Hugging Face repository in `owner/name` form. Identifies the
    /// package's primary catalog identity.
    pub repo: String,
    /// Package directory; defaults to `./<repo-name>` under the working directory.
    #[arg(long, value_name = "DIR")]
    pub path: Option<PathBuf>,
    #[arg(long, default_value = "v1")]
    pub version: String,
    #[arg(long, default_value = "dataset")]
    pub kind: String,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long = "metadata", value_name = "KEY=VALUE")]
    pub metadata: Vec<String>,
    #[arg(long)]
    pub force: bool,
}

pub fn run_init(args: InitArgs) -> Result<()> {
    let default_name = args.repo.rsplit_once('/').map(|(_, name)| name.to_string())
        .unwrap_or_else(|| args.repo.clone());
    let target = args.path.unwrap_or_else(|| PathBuf::from(&default_name));
    let mut metadata = parse_metadata(&args.metadata)?;
    if let Some(description) = &args.description {
        metadata.insert("description".into(), description.clone());
    }
    let spec = PackageSpec {
        schema_version: 1,
        repo: args.repo.clone(),
        version: args.version,
        kind: args.kind,
        metadata,
        payload: serde_json::Map::new(),
    };

    let staging =
        tempfile::tempdir().map_err(|error| format!("create staging directory: {error}"))?;
    let readme = format!(
        "# {repo}\n\nVersion: {version}\nKind: {kind}\n\n\
         This file is part of the package payload. Add data files beside it, then rebuild with:\n\n\
         autonomics-catalog build <package> <output> --force\n\n\
         Replace or remove this README if it should not be distributed with the data.\n",
        repo = spec.repo,
        version = spec.version,
        kind = spec.kind,
    );
    std::fs::write(staging.path().join("README.md"), readme)
        .map_err(|error| format!("write seed payload: {error}"))?;

    build_package(
        staging.path(),
        &target,
        BuildOptions {
            repo: Some(spec.repo.clone()),
            version: Some(spec.version.clone()),
            kind: Some(spec.kind.clone()),
            metadata: spec.metadata.clone(),
            payload: spec.payload.clone(),
            force: args.force,
        },
    )
    .map_err(|error| error.to_string())?;

    let spec_bytes = serde_json::to_vec_pretty(&spec)
        .map_err(|error| format!("encode package spec: {error}"))?;
    std::fs::write(target.join(PACKAGE_SPEC), spec_bytes)
        .map_err(|error| format!("write package spec: {error}"))?;

    validate_package(&target).map_err(|error| error.to_string())?;
    println!(
        "created data package {}@{} at {}",
        spec.repo,
        spec.version,
        target.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_valid_rebuildable_package() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("demo.panel");
        run_init(InitArgs {
            repo: "wjixiang/catalog-demo-panel".into(),
            path: Some(target.clone()),
            version: "v1".into(),
            kind: "panel".into(),
            description: Some("demo panel".into()),
            metadata: Vec::new(),
            force: false,
        })
        .unwrap();

        let manifest = validate_package(&target).unwrap();
        assert_eq!(manifest.repo, "wjixiang/catalog-demo-panel");
        assert_eq!(
            manifest.metadata.get("description").map(String::as_str),
            Some("demo panel")
        );
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "README.md");
        assert!(target.join(PACKAGE_SPEC).is_file());

        let rebuilt = directory.path().join("rebuilt");
        build_package(&target, &rebuilt, BuildOptions::default()).unwrap();
        let rebuilt_manifest = validate_package(&rebuilt).unwrap();
        assert_eq!(rebuilt_manifest.repo, "wjixiang/catalog-demo-panel");
        assert_eq!(
            rebuilt_manifest
                .metadata
                .get("description")
                .map(String::as_str),
            Some("demo panel")
        );
    }

    #[test]
    fn init_defaults_to_working_directory() {
        let original = std::env::current_dir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        std::env::set_current_dir(directory.path()).unwrap();
        let result = run_init(InitArgs {
            repo: "wjixiang/catalog-pwd-panel".into(),
            path: None,
            version: "v1".into(),
            kind: "panel".into(),
            description: None,
            metadata: Vec::new(),
            force: false,
        });
        std::env::set_current_dir(&original).unwrap();
        result.unwrap();

        let target = directory.path().join("catalog-pwd-panel");
        assert!(validate_package(&target).is_ok());
    }
}
