use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

const DEFAULT_RESOURCE_ROOT: &str = "/mnt/data/mixer/resources";
const DEFAULT_REFERENCE_ID: &str = "g1000_eur";
const BUNDLE_MANIFEST: &str = "bundle.json";

#[derive(Debug)]
pub(crate) struct MixerReferenceBundle {
    pub(crate) mixer_home: PathBuf,
    pub(crate) bim_template: String,
    pub(crate) ld_template: String,
    pub(crate) extract_template: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MixerPanelBundle {
    schema_version: u8,
    id: String,
    source_commit: String,
    genome_build: String,
    population: String,
    engine_path: String,
    bim_template: String,
    ld_template: String,
    extract_template: String,
    engine_sha256: String,
}

pub(crate) fn resource_root() -> PathBuf {
    std::env::var("MIXER_RESOURCE_ROOT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_RESOURCE_ROOT))
}

pub(crate) fn default_reference() -> String {
    DEFAULT_REFERENCE_ID.to_string()
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn bundle_path(reference: &str, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!(
            "bundle path '{relative}' must be relative and cannot escape the bundle"
        ));
    }
    Ok(resource_root().join(reference).join(relative))
}

fn validate_template(reference: &str, value: &str) -> Result<(), String> {
    if value.matches('@').count() != 1 {
        return Err(format!(
            "template '{value}' must contain exactly one chromosome placeholder (@)"
        ));
    }
    bundle_path(reference, value)?;
    Ok(())
}

pub(crate) fn resolve_reference(reference: &str) -> Result<MixerReferenceBundle, String> {
    if !valid_id(reference) {
        return Err(format!(
            "invalid reference ID '{reference}': IDs may contain only ASCII letters, digits, '_', '-', and '.'"
        ));
    }

    let manifest_path = resource_root().join(reference).join(BUNDLE_MANIFEST);
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;
    let bundle: MixerPanelBundle = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| format!("invalid {BUNDLE_MANIFEST} for {reference}: {e}"))?;

    if bundle.schema_version != 1 {
        return Err(format!(
            "unsupported bundle schema version {}",
            bundle.schema_version
        ));
    }
    if bundle.id != reference {
        return Err(format!(
            "bundle ID '{}' does not match requested reference '{reference}'",
            bundle.id
        ));
    }
    for (name, value) in [
        ("source_commit", &bundle.source_commit),
        ("genome_build", &bundle.genome_build),
        ("population", &bundle.population),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{name} cannot be empty"));
        }
    }
    validate_template(reference, &bundle.bim_template)?;
    validate_template(reference, &bundle.ld_template)?;
    validate_template(reference, &bundle.extract_template)?;

    let mixer_home = bundle_path(reference, &bundle.engine_path)?;
    let mixer_py = mixer_home.join("precimed").join("mixer.py");
    let library = mixer_home.join("libbgmg.so");
    if !mixer_py.is_file() {
        return Err(format!("missing mixer engine: {}", mixer_py.display()));
    }
    let library_bytes =
        std::fs::read(&library).map_err(|e| format!("read {}: {e}", library.display()))?;
    let actual_checksum = Sha256::digest(&library_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if bundle.engine_sha256.len() != 64
        || bundle
            .engine_sha256
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit())
    {
        return Err("engine_sha256 must contain 64 hexadecimal characters".into());
    }
    if !actual_checksum.eq_ignore_ascii_case(&bundle.engine_sha256) {
        return Err(format!(
            "libbgmg.so checksum mismatch: expected {}, got {actual_checksum}",
            bundle.engine_sha256
        ));
    }

    Ok(MixerReferenceBundle {
        mixer_home,
        bim_template: bundle_path(reference, &bundle.bim_template)?
            .to_string_lossy()
            .into_owned(),
        ld_template: bundle_path(reference, &bundle.ld_template)?
            .to_string_lossy()
            .into_owned(),
        extract_template: bundle_path(reference, &bundle.extract_template)?
            .to_string_lossy()
            .into_owned(),
    })
}

pub(crate) fn python_executable(mixer_home: &Path) -> String {
    std::env::var("MIXER_PYTHON")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            mixer_home
                .join(".venv/bin/python")
                .to_string_lossy()
                .into_owned()
        })
}
