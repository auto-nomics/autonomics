use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::model::{
    DatasetFile, DatasetManifest, manifest_digest, validate_id, validate_kind,
    validate_relative_path, validate_version,
};

pub const PACKAGE_MANIFEST: &str = "manifest.json";
pub const PACKAGE_SPEC: &str = "package.json";
pub const PAYLOAD_DIR: &str = "payload";

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("package input/output error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid package: {0}")]
    Invalid(String),
    #[error("invalid package spec: {0}")]
    Spec(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub id: Option<String>,
    pub version: Option<String>,
    pub kind: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub payload: serde_json::Map<String, serde_json::Value>,
    pub force: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PackageSpec {
    schema_version: u8,
    id: String,
    version: String,
    kind: String,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    #[serde(default)]
    payload: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
pub struct BuiltPackage {
    pub path: PathBuf,
    pub manifest: DatasetManifest,
}

#[derive(Debug, Clone)]
struct PreparedInput {
    payload_root: PathBuf,
    spec: Option<PackageSpec>,
}

pub fn build_package(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: BuildOptions,
) -> Result<BuiltPackage, PackageError> {
    let input = input.as_ref().canonicalize()?;
    let output = output.as_ref();
    if output.exists() && !options.force {
        return Err(PackageError::Invalid(format!(
            "output already exists: {} (use force to replace it)",
            output.display()
        )));
    }
    let output_parent = output
        .parent()
        .ok_or_else(|| PackageError::Invalid("output has no parent".into()))?
        .to_path_buf();
    fs::create_dir_all(&output_parent)?;
    let output_parent = output_parent.canonicalize()?;
    let output = output_parent.join(output.file_name().ok_or_else(|| {
        PackageError::Invalid(format!("output has no file name: {}", output.display()))
    })?);
    if output.starts_with(&input) {
        return Err(PackageError::Invalid(
            "output cannot be inside the input package".into(),
        ));
    }

    let prepared = prepare_input(&input)?;
    let spec = resolve_spec(prepared.spec.as_ref(), &options)?;
    let staging = tempfile::tempdir_in(&output_parent)?;
    let payload_destination = staging.path().join(PAYLOAD_DIR);
    fs::create_dir_all(&payload_destination)?;

    let mut files = Vec::new();
    let mut stack = vec![prepared.payload_root.clone()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(PackageError::Invalid(format!(
                    "package payload contains a non-regular file: {}",
                    path.display()
                )));
            }
            let relative = path
                .strip_prefix(&prepared.payload_root)
                .map_err(|_| PackageError::Invalid("payload path escapes package".into()))?
                .to_string_lossy()
                .replace('\\', "/");
            validate_relative_path(&relative).map_err(PackageError::Invalid)?;
            let destination = payload_destination.join(&relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &destination)?;
            let checksum = file_sha256(&destination)?;
            files.push(DatasetFile {
                path: relative,
                size: metadata.len(),
                sha256: checksum,
            });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.is_empty() {
        return Err(PackageError::Invalid("package has no payload files".into()));
    }

    let mut manifest = DatasetManifest {
        schema_version: crate::model::DATASET_SCHEMA_VERSION,
        id: spec.id,
        version: spec.version,
        kind: spec.kind,
        metadata: spec.metadata,
        files,
        payload: spec.payload,
        digest: None,
    };
    manifest.digest = Some(manifest_digest(&manifest));
    manifest.validate().map_err(PackageError::Invalid)?;
    fs::write(
        staging.path().join(PACKAGE_MANIFEST),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    if options.force && output.exists() {
        if output.is_dir() {
            fs::remove_dir_all(&output)?;
        } else {
            fs::remove_file(&output)?;
        }
    }
    fs::rename(staging.path(), &output)?;
    Ok(BuiltPackage {
        path: output,
        manifest,
    })
}

pub fn validate_package(path: impl AsRef<Path>) -> Result<DatasetManifest, PackageError> {
    let path = path.as_ref().canonicalize()?;
    let manifest_path = path.join(PACKAGE_MANIFEST);
    let manifest: DatasetManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    manifest.validate().map_err(PackageError::Invalid)?;
    for file in &manifest.files {
        let payload = path.join(PAYLOAD_DIR).join(&file.path);
        let metadata = fs::metadata(&payload).map_err(|error| {
            PackageError::Invalid(format!("missing payload `{}`: {error}", file.path))
        })?;
        if !metadata.is_file() || metadata.len() != file.size {
            return Err(PackageError::Invalid(format!(
                "payload size mismatch for `{}`",
                file.path
            )));
        }
        let checksum = file_sha256(&payload)?;
        if checksum != file.sha256 {
            return Err(PackageError::Invalid(format!(
                "payload checksum mismatch for `{}`",
                file.path
            )));
        }
    }
    Ok(manifest)
}

fn prepare_input(input: &Path) -> Result<PreparedInput, PackageError> {
    if input.is_dir() {
        return package_or_directory(input.to_path_buf());
    }
    if !input.is_file() {
        return Err(PackageError::Invalid(format!(
            "input is neither a file nor directory: {}",
            input.display()
        )));
    }

    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if extension == "tar" || stem.ends_with(".tar") {
        let root = extract_archive(input)?;
        return package_or_directory(root);
    }

    let root = tempfile::tempdir()?.keep();
    let payload_root = root.join("payload");
    fs::create_dir_all(&payload_root)?;
    let filename = input
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| PackageError::Invalid("input filename is not UTF-8".into()))?;
    fs::copy(input, payload_root.join(filename))?;
    Ok(PreparedInput {
        payload_root,
        spec: None,
    })
}

fn package_or_directory(root: PathBuf) -> Result<PreparedInput, PackageError> {
    let manifest = root.join(PACKAGE_MANIFEST);
    let payload = root.join(PAYLOAD_DIR);
    if manifest.is_file() && payload.is_dir() {
        let spec = if root.join(PACKAGE_SPEC).is_file() {
            Some(serde_json::from_slice::<PackageSpec>(&fs::read(
                root.join(PACKAGE_SPEC),
            )?)?)
        } else {
            None
        };
        return Ok(PreparedInput {
            payload_root: payload,
            spec,
        });
    }

    // unwrap a single top-level directory produced by `tar -C parent dir`.
    let entries: Vec<_> = fs::read_dir(&root)?.collect::<Result<Vec<_>, _>>()?;
    if entries.len() == 1 && entries[0].path().is_dir() {
        let child = entries[0].path();
        if child.join(PACKAGE_MANIFEST).is_file() && child.join(PAYLOAD_DIR).is_dir() {
            let spec = if child.join(PACKAGE_SPEC).is_file() {
                Some(serde_json::from_slice::<PackageSpec>(&fs::read(
                    child.join(PACKAGE_SPEC),
                )?)?)
            } else {
                None
            };
            return Ok(PreparedInput {
                payload_root: child.join(PAYLOAD_DIR),
                spec,
            });
        }
    }
    Ok(PreparedInput {
        payload_root: root,
        spec: None,
    })
}

fn extract_archive(input: &Path) -> Result<PathBuf, PackageError> {
    let destination = tempfile::tempdir()?.keep();
    fs::create_dir_all(&destination)?;
    let file = fs::File::open(input)?;
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();

    if extension == "zst" || stem.ends_with(".tar") {
        let decoder = zstd::Decoder::new(file)?;
        unpack_tar(decoder, &destination)?;
    } else if extension == "gz" || extension == "tgz" || stem.ends_with(".tar") {
        let decoder = flate2::read::GzDecoder::new(file);
        unpack_tar(decoder, &destination)?;
    } else {
        unpack_tar(file, &destination)?;
    }
    Ok(destination)
}

fn unpack_tar<R: Read>(reader: R, destination: &Path) -> Result<(), PackageError> {
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(false);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let metadata = entry.header().entry_type();
        if metadata.is_symlink() || metadata.is_hard_link() {
            return Err(PackageError::Invalid(
                "package archives may not contain links".into(),
            ));
        }
        let relative = entry
            .path()?
            .to_path_buf()
            .to_string_lossy()
            .replace('\\', "/");
        validate_relative_path(&relative).map_err(PackageError::Invalid)?;
        let target = destination.join(
            Path::new(&relative)
                .components()
                .filter_map(|component| match component {
                    Component::Normal(value) => Some(value),
                    _ => None,
                })
                .collect::<PathBuf>(),
        );
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&target)?;
    }
    Ok(())
}

fn resolve_spec(
    spec: Option<&PackageSpec>,
    options: &BuildOptions,
) -> Result<PackageSpec, PackageError> {
    let mut resolved = spec.cloned().unwrap_or(PackageSpec {
        schema_version: 1,
        id: String::new(),
        version: String::new(),
        kind: String::new(),
        metadata: BTreeMap::new(),
        payload: serde_json::Map::new(),
    });
    if resolved.schema_version != 1 {
        return Err(PackageError::Invalid(
            "unsupported package spec schema version".into(),
        ));
    }
    if let Some(id) = &options.id {
        resolved.id = id.clone();
    }
    if let Some(version) = &options.version {
        resolved.version = version.clone();
    }
    if let Some(kind) = &options.kind {
        resolved.kind = kind.clone();
    }
    resolved.metadata.extend(options.metadata.clone());
    for (key, value) in options.payload.clone() {
        resolved.payload.insert(key, value);
    }
    validate_id(&resolved.id).map_err(PackageError::Invalid)?;
    validate_version(&resolved.version).map_err(PackageError::Invalid)?;
    validate_kind(&resolved.kind).map_err(PackageError::Invalid)?;
    Ok(resolved)
}

fn file_sha256(path: &Path) -> Result<String, PackageError> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{}", crate::model::hex(&hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_validates_directory_package() {
        let input = tempfile::tempdir().unwrap();
        let nested = input.path().join("chr22");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("panel.vcf.gz"), b"vcf").unwrap();
        let output_parent = tempfile::tempdir().unwrap();
        let output = output_parent.path().join("package");

        let built = build_package(
            input.path(),
            &output,
            BuildOptions {
                id: Some("1000g_eur".into()),
                version: Some("v3".into()),
                kind: Some("vcf".into()),
                metadata: BTreeMap::from([("population".into(), "EUR".into())]),
                payload: serde_json::Map::new(),
                force: false,
            },
        )
        .unwrap();

        assert!(output.join("manifest.json").is_file());
        assert!(output.join("payload/chr22/panel.vcf.gz").is_file());
        assert_eq!(built.manifest.id, "1000g_eur");
        let validated = validate_package(&output).unwrap();
        assert_eq!(validated, built.manifest);
    }

    #[test]
    fn rejects_tampered_payload() {
        let input = tempfile::tempdir().unwrap();
        fs::write(input.path().join("data.txt"), b"ok").unwrap();
        let output_parent = tempfile::tempdir().unwrap();
        let output = output_parent.path().join("package");
        build_package(
            input.path(),
            &output,
            BuildOptions {
                id: Some("panel".into()),
                version: Some("v1".into()),
                kind: Some("table".into()),
                ..Default::default()
            },
        )
        .unwrap();
        fs::write(output.join("payload/data.txt"), b"no").unwrap();

        let error = validate_package(&output).unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"));
    }

    #[test]
    fn builds_from_tar_archive() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("source.tar");
        let mut archive = tar::Builder::new(fs::File::create(&source).unwrap());
        let mut header = tar::Header::new_gnu();
        let content = b"archive-panel";
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "chr22/panel.txt", content.as_slice())
            .unwrap();
        archive.into_inner().unwrap();

        let output = workspace.path().join("package");
        let built = build_package(
            source,
            &output,
            BuildOptions {
                id: Some("archive_panel".into()),
                version: Some("v1".into()),
                kind: Some("plink".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(built.manifest.files[0].path, "chr22/panel.txt");
        assert!(output.join("payload/chr22/panel.txt").is_file());
    }
}
