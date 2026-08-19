use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

const DEFAULT_RESOURCE_ROOTS: [&str; 2] = ["/data/mixer/resources", "/mnt/data/mixer/resources"];
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

fn resource_roots_from_override(value: Option<&str>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        roots.extend(
            value
                .split(':')
                .filter(|candidate| !candidate.trim().is_empty())
                .map(|candidate| PathBuf::from(candidate.trim())),
        );
    }

    for default_root in DEFAULT_RESOURCE_ROOTS {
        let default_root = PathBuf::from(default_root);
        if !roots.contains(&default_root) {
            roots.push(default_root);
        }
    }
    roots
}

fn resource_roots() -> Vec<PathBuf> {
    resource_roots_from_override(std::env::var("MIXER_RESOURCE_ROOT").ok().as_deref())
}

pub(crate) fn default_reference() -> String {
    DEFAULT_REFERENCE_ID.to_string()
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn bundle_path(root: &Path, reference: &str, relative: &str) -> Result<PathBuf, String> {
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
    Ok(root.join(reference).join(relative))
}

fn validate_template(root: &Path, reference: &str, value: &str) -> Result<(), String> {
    if value.matches('@').count() != 1 {
        return Err(format!(
            "template '{value}' must contain exactly one chromosome placeholder (@)"
        ));
    }
    bundle_path(root, reference, value)?;
    Ok(())
}

fn resolve_reference_at(root: PathBuf, reference: &str) -> Result<MixerReferenceBundle, String> {
    if !valid_id(reference) {
        return Err(format!(
            "invalid reference ID '{reference}': IDs may contain only ASCII letters, digits, '_', '-', and '.'"
        ));
    }

    let manifest_path = root.join(reference).join(BUNDLE_MANIFEST);
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
    validate_template(&root, reference, &bundle.bim_template)?;
    validate_template(&root, reference, &bundle.ld_template)?;
    validate_template(&root, reference, &bundle.extract_template)?;

    let mixer_home = bundle_path(&root, reference, &bundle.engine_path)?;
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
        bim_template: bundle_path(&root, reference, &bundle.bim_template)?
            .to_string_lossy()
            .into_owned(),
        ld_template: bundle_path(&root, reference, &bundle.ld_template)?
            .to_string_lossy()
            .into_owned(),
        extract_template: bundle_path(&root, reference, &bundle.extract_template)?
            .to_string_lossy()
            .into_owned(),
    })
}

pub(crate) fn resolve_reference(reference: &str) -> Result<MixerReferenceBundle, String> {
    if !valid_id(reference) {
        return Err(format!(
            "invalid reference ID '{reference}': IDs may contain only ASCII letters, digits, '_', '-', and '.'"
        ));
    }

    let roots = resource_roots();
    let mut failures = Vec::with_capacity(roots.len());
    for root in &roots {
        match resolve_reference_at(root.clone(), reference) {
            Ok(bundle) => return Ok(bundle),
            Err(error) => failures.push(error),
        }
    }

    let candidates = roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "no valid MiXeR reference '{reference}' under resource roots [{candidates}]: {}",
        failures.join("; ")
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_resource_roots_default_to_canonical_then_legacy_path() {
        let roots = resource_roots_from_override(None);
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/data/mixer/resources"),
                PathBuf::from("/mnt/data/mixer/resources"),
            ]
        );
    }

    #[test]
    fn mixer_resource_root_override_accepts_candidates() {
        let roots = resource_roots_from_override(Some(" /tmp/mixer :/opt/mixer :: "));
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/tmp/mixer"),
                PathBuf::from("/opt/mixer"),
                PathBuf::from("/data/mixer/resources"),
                PathBuf::from("/mnt/data/mixer/resources"),
            ]
        );
    }
}
