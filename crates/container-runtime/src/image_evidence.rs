//! Exact image metadata from the coordinator's explicitly configured store.
//!
//! This never invokes Podman (even inspect can initialize runtime state), opens
//! a database, mounts a rootfs, pulls, or writes to the store. Content addresses
//! authenticate metadata bytes; repository association is a local observation,
//! not a registry signature or qualification of layer bytes/dependencies.

use std::{
    ffi::CString,
    fs::File,
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{ImageReference, PodmanConfig};

const MAX_INDEX: u64 = 32 * 1024 * 1024;
const MAX_BLOB: u64 = 4 * 1024 * 1024;

/// JSON strings preserve the exact UTF-8 bytes, including whitespace/order.
/// Only the receipt is suitable for routine logs; config can contain image env.
#[derive(Debug, Serialize, Deserialize)]
pub struct ImageEvidence {
    pub receipt: Value,
    pub manifest_json: String,
    pub config_json: String,
}

fn require(ok: bool, message: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message.into()) }
}

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn hex_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(hex_id)
}

fn key_filename(key: &str) -> String {
    // containers/storage uses verbatim safe names, otherwise '=' + base64.
    format!("={}", STANDARD.encode(key))
}

fn open_child(parent: &File, name: &str, directory: bool) -> Result<File, String> {
    require(
        !name.is_empty() && name != "." && name != ".." && !name.contains('/'),
        "invalid store component",
    )?;
    let name = CString::new(name).map_err(|e| e.to_string())?;
    // Linux ancestors can legitimately be searchable but not listable (e.g.
    // /dssg/home on HPC). A path descriptor pins their identity without asking
    // for directory read permission. Blob/index descriptors stay O_RDONLY.
    #[cfg(target_os = "linux")]
    let directory_access = libc::O_PATH;
    #[cfg(not(target_os = "linux"))]
    let directory_access = libc::O_RDONLY;
    let flags = libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory {
            directory_access | libc::O_DIRECTORY
        } else {
            libc::O_RDONLY
        };
    // SAFETY: parent is a live directory descriptor; name is NUL terminated.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(format!(
            "cannot read store component: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn directory(path: &Path) -> Result<File, String> {
    require(
        path.is_absolute(),
        "explicit absolute private storage root required",
    )?;
    let mut current = File::open("/").map_err(|e| e.to_string())?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = open_child(
                    &current,
                    name.to_str().ok_or("non-UTF8 store component")?,
                    true,
                )?
            }
            _ => return Err("noncanonical store path".into()),
        }
    }
    Ok(current)
}

fn stamp(m: &std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

fn read(parent: &File, name: &str, maximum: u64) -> Result<(Vec<u8>, Value), String> {
    let mut file = open_child(parent, name, false)?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    require(
        before.is_file() && before.len() <= maximum,
        "bounded regular store file required",
    )?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    let named = open_child(parent, name, false)?
        .metadata()
        .map_err(|e| e.to_string())?;
    require(
        stamp(&before) == stamp(&after)
            && stamp(&before) == stamp(&named)
            && bytes.len() as u64 == before.len(),
        "store file changed while reading",
    )?;
    let identity = json!({"sha256":format!("{:x}", Sha256::digest(&bytes)),"bytes":bytes.len(),"device":before.dev(),"inode":before.ino()});
    Ok((bytes, identity))
}

fn repository(name: &str) -> &str {
    let name = name.split('@').next().unwrap_or(name);
    let slash = name.rfind('/').map_or(0, |i| i + 1);
    if let Some(colon) = name[slash..].rfind(':') {
        &name[..slash + colon]
    } else {
        name
    }
}

/// Inspect only this backend; absence/errors never fall back to another host,
/// storage driver, tag, image ID, remote registry, or worker's cache.
pub fn inspect(config: &PodmanConfig, reference: &str) -> Result<ImageEvidence, String> {
    let parsed = ImageReference::parse(reference)?;
    require(
        parsed.as_str() == reference,
        "canonical digest-pinned reference required",
    )?;
    let (repo, wanted) = reference
        .rsplit_once('@')
        .ok_or("digest-pinned reference required")?;
    let driver = config
        .storage_driver
        .as_deref()
        .ok_or("explicit storage driver required")?;
    require(
        matches!(driver, "overlay" | "vfs"),
        "unsupported private storage driver",
    )?;
    let root_path = config
        .storage_root
        .as_deref()
        .ok_or("explicit private storage root required")?;
    let root = directory(root_path)?;
    let images = open_child(&root, &format!("{driver}-images"), true)?;
    let (index_bytes, index_identity) = read(&images, "images.json", MAX_INDEX)?;
    let rows: Vec<Value> =
        serde_json::from_slice(&index_bytes).map_err(|e| format!("invalid image index: {e}"))?;
    let key = format!("manifest-{wanted}");
    let matches: Vec<_> = rows
        .iter()
        .filter(|r| {
            let named = ["names", "names-history"].iter().any(|k| {
                r[*k].as_array().is_some_and(|names| {
                    names
                        .iter()
                        .any(|n| n.as_str().is_some_and(|n| repository(n) == repo))
                })
            });
            named
                && (r["digest"].as_str() == Some(wanted)
                    || r["big-data-digests"][&key].as_str() == Some(wanted))
        })
        .collect();
    require(
        matches.len() == 1,
        "exact repository/digest mapping missing or ambiguous in configured store",
    )?;
    let row = matches[0];
    let id = row["id"].as_str().ok_or("image ID missing")?;
    require(hex_id(id), "invalid image ID")?;
    let image = open_child(&images, id, true)?;
    let big_names = row["big-data-names"]
        .as_array()
        .ok_or("image big-data names missing")?;
    let manifest_key = if big_names.iter().any(|v| v.as_str() == Some(&key)) {
        key
    } else {
        "manifest".into()
    };
    let manifest_file = if manifest_key == "manifest" {
        "manifest".into()
    } else {
        key_filename(&manifest_key)
    };
    let (manifest_bytes, manifest_identity) = read(&image, &manifest_file, MAX_BLOB)?;
    require(
        sha(&manifest_bytes) == wanted,
        "stored manifest bytes do not match pinned digest",
    )?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes).map_err(|e| e.to_string())?;
    require(
        manifest["schemaVersion"] == 2
            && matches!(
                manifest["mediaType"].as_str(),
                Some(
                    "application/vnd.oci.image.manifest.v1+json"
                        | "application/vnd.docker.distribution.manifest.v2+json"
                )
            ),
        "single-image OCI/Docker schema-2 manifest required (indexes are not resolved implicitly)",
    )?;
    let config_digest = manifest["config"]["digest"]
        .as_str()
        .ok_or("manifest config digest missing")?;
    require(
        digest(config_digest) && &config_digest[7..] == id,
        "manifest config differs from store image ID",
    )?;
    let (config_bytes, config_identity) = read(&image, &key_filename(config_digest), MAX_BLOB)?;
    require(
        sha(&config_bytes) == config_digest
            && manifest["config"]["size"].as_u64() == Some(config_bytes.len() as u64),
        "stored config bytes do not match manifest descriptor",
    )?;
    let image_config: Value = serde_json::from_slice(&config_bytes).map_err(|e| e.to_string())?;
    let layers = manifest["layers"]
        .as_array()
        .ok_or("manifest layers missing")?;
    let diff_ids = image_config["rootfs"]["diff_ids"]
        .as_array()
        .ok_or("config diff IDs missing")?;
    require(
        image_config["rootfs"]["type"] == "layers" && layers.len() == diff_ids.len(),
        "manifest/config layer count differs",
    )?;
    for (layer, diff) in layers.iter().zip(diff_ids) {
        require(
            layer["digest"].as_str().is_some_and(digest)
                && layer["size"].as_u64().is_some()
                && diff.as_str().is_some_and(digest),
            "invalid layer descriptor/diff ID",
        )?;
        let kind = layer["mediaType"]
            .as_str()
            .ok_or("layer media type missing")?;
        require(
            matches!(
                kind,
                "application/vnd.oci.image.layer.v1.tar"
                    | "application/vnd.oci.image.layer.v1.tar+gzip"
                    | "application/vnd.oci.image.layer.v1.tar+zstd"
                    | "application/vnd.docker.image.rootfs.diff.tar.gzip"
            ),
            "unsupported layer encoding",
        )?;
        if kind == "application/vnd.oci.image.layer.v1.tar" {
            require(
                layer["digest"] == *diff,
                "uncompressed layer descriptor differs from config diff ID",
            )?;
        }
    }
    let (index_after, _) = read(&images, "images.json", MAX_INDEX)?;
    require(
        index_bytes == index_after,
        "store index changed during observation; retry read",
    )?;
    let receipt = json!({
        "schema":"COORDINATOR_IMAGE_METADATA_EVIDENCE_V1",
        "scope":"EXACT_MANIFEST_CONFIG_METADATA_ONLY",
        "reference":reference,"storage_root":root_path,"storage_driver":driver,
        "image_id":id,"manifest_digest":wanted,"config_digest":config_digest,
        "source_repository_mapping":{
            "repository":repo,"recorded_digest":row["digest"],
            "names":row["names"],"names_history":row["names-history"],
            "manifest_key":manifest_key
        },
        "source_index":index_identity,"source_manifest_file":manifest_file,
        "manifest":manifest_identity,"config":config_identity,
        "layer_count":layers.len(),"layers":layers,"diff_ids":diff_ids,
        "repository_mapping_observed":true,"manifest_config_binding_verified":true,
        "layer_bytes_verified":false,"archive_export_verified":false,
        "sif_mapping_verified":false,"dependencies_qualified":false,
        "scientific_authorization":false,"store_mutated":false
    });
    Ok(ImageEvidence {
        receipt,
        manifest_json: String::from_utf8(manifest_bytes).map_err(|e| e.to_string())?,
        config_json: String::from_utf8(config_bytes).map_err(|e| e.to_string())?,
    })
}

#[cfg(test)]
mod tests;
