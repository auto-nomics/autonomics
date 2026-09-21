use std::path::{Path, PathBuf};

use data_catalog::error::Result;
use data_catalog::{CatalogConfig, LocalCatalog, RemoteCatalog};

pub(crate) fn expand_home(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref().to_path_buf();
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        let home = std::env::var_os("HOME").ok_or_else(|| "$HOME is not set".to_string())?;
        return Ok(Path::new(&home).join(rest));
    }
    Ok(path)
}

pub(crate) fn load_catalog_config(config: &Path) -> Result<CatalogConfig> {
    let source = std::fs::read_to_string(expand_home(config)?)
        .map_err(|error| format!("read VFS config: {error}"))?;
    CatalogConfig::from_vfs_toml(&source)
}

pub(crate) async fn load_remote(config: &Path) -> Result<RemoteCatalog> {
    let catalog_config = load_catalog_config(config)?;
    let repository = catalog_config
        .repository
        .as_deref()
        .ok_or_else(|| "catalog repository is required".to_string())?;
    RemoteCatalog::hf(
        repository,
        catalog_config.revision.clone(),
        data_catalog::hf::resolve_hf_token(None),
    )
}

pub(crate) fn open_cache(path: &Path) -> Result<LocalCatalog> {
    LocalCatalog::open(expand_home(path)?)
}

/// Default package cache: the shared panel cache root.
pub(crate) fn default_cache_root() -> String {
    data_catalog::default_panel_cache_root()
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn parse_metadata(
    values: &[String],
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut metadata = std::collections::BTreeMap::new();
    for value in values {
        let (key, value) = value
            .split_once('=')
            .ok_or_else(|| format!("metadata must be KEY=VALUE, got `{value}`"))?;
        metadata.insert(key.to_string(), value.to_string());
    }
    Ok(metadata)
}

pub(crate) fn print_json(value: &impl serde::Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    println!("{}", String::from_utf8_lossy(&bytes));
    Ok(())
}
