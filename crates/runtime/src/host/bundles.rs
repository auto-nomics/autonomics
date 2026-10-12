//! User-provided data bundle registry (`state_dir/data_bundles.toml`).

use dag_core::{BundleRegistry, DataBundle};

use crate::config::RuntimeConfig;
use crate::error::{Error, Result};

#[derive(Debug, serde::Deserialize)]
struct DataBundleManifest {
    #[serde(default)]
    bundle: Vec<DataBundle>,
}

/// Load user-provided bundle registry entries from `state_dir/data_bundles.toml`.
///
/// A missing file yields an empty registry. Entries returned here override
/// Hugging Face catalog entries, while built-in entries are layered beneath
/// both when the engine is constructed.
pub(super) fn build_bundle_registry(config: &RuntimeConfig) -> Result<BundleRegistry> {
    let manifest_path = config.state_dir.join("data_bundles.toml");
    let source = match std::fs::read_to_string(&manifest_path) {
        Ok(source) => source,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BundleRegistry::new());
        }
        Err(e) => {
            return Err(Error::Other(format!(
                "read data bundle manifest {}: {e}",
                manifest_path.display()
            )));
        }
    };
    let manifest: DataBundleManifest = toml::from_str(&source)
        .map_err(|e| Error::Other(format!("invalid {}: {e}", manifest_path.display())))?;
    BundleRegistry::from_bundles(manifest.bundle)
        .map_err(|e| Error::Other(format!("invalid {}: {e}", manifest_path.display())))
}

#[cfg(test)]
mod bundle_registry_tests {
    use super::*;

    #[test]
    fn loads_bundle_ids_from_state_manifest() {
        let state = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::default();
        config.state_dir = state.path().to_path_buf();
        std::fs::write(
            state.path().join("data_bundles.toml"),
            r#"
[[bundle]]
ident = "EUR.panel"
desc = "1000G EUR reference panel"
vpath = "/bundles/panels/EUR.panel"
"#,
        )
        .unwrap();

        let registry = build_bundle_registry(&config).unwrap();

        assert_eq!(
            registry
                .get("EUR.panel")
                .map(|bundle| bundle.vpath.as_str())
                .unwrap(),
            "/bundles/panels/EUR.panel"
        );
    }
}
