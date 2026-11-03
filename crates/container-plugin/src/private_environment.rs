use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::manifest::PluginManifest;

const MAX_POLICY_BYTES: u64 = 65536;
const FAMILY: &str = "ieugwasr-official";
const KINDS: [&str; 2] = [
    "official_ieugwasr_call",
    "official_ieugwasr_call_with_dataset",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema: u32,
    grants: Vec<Grant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    plugin: String,
    kind: String,
    manifest_sha256: String,
    script_sha256: String,
    image_reference: String,
    names: Vec<String>,
}

fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl Policy {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let policy: Self = serde_json::from_slice(bytes)
            .map_err(|_| "invalid private environment policy schema".to_string())?;
        if policy.schema != 1 || policy.grants.len() > KINDS.len() {
            return Err("unsupported private environment policy schema or grant count".into());
        }
        let mut addresses = BTreeSet::new();
        for grant in &policy.grants {
            if grant.plugin != FAMILY
                || !KINDS.contains(&grant.kind.as_str())
                || grant.names != ["OPENGWAS_JWT"]
                || !valid_digest(&grant.manifest_sha256)
                || !valid_digest(&grant.script_sha256)
                || !grant
                    .image_reference
                    .strip_prefix("ghcr.io/zj-2002/ieugwasr-official@")
                    .is_some_and(valid_digest)
                || !addresses.insert((&grant.plugin, &grant.kind))
            {
                return Err("unsupported or duplicate private environment authorization".into());
            }
        }
        Ok(policy)
    }

    fn authorize(
        &self,
        manifest: &PluginManifest,
        manifest_bytes: &[u8],
    ) -> Result<BTreeMap<String, Vec<String>>, String> {
        let mut authorized = BTreeMap::new();
        for grant in &self.grants {
            if grant.plugin != manifest.plugin_name {
                continue;
            }
            let node = manifest
                .nodes
                .iter()
                .find(|node| node.kind == grant.kind)
                .ok_or("private environment policy references an absent node kind")?;
            let script = node
                .command
                .script
                .as_deref()
                .ok_or("private environment policy requires a loaded script")?;
            if grant.manifest_sha256 != hash(manifest_bytes)
                || grant.script_sha256 != hash(script.as_bytes())
                || grant.image_reference != manifest.image.reference.as_str()
            {
                return Err(
                    "private environment authorization implementation identity mismatch".into(),
                );
            }
            authorized.insert(grant.kind.clone(), grant.names.clone());
        }
        Ok(authorized)
    }
}

#[cfg(target_os = "linux")]
fn read_owner_policy(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let information = match std::fs::symlink_metadata(path) {
        Ok(information) => information,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot inspect private environment policy".into()),
    };
    let owner = std::fs::metadata("/proc/self")
        .map_err(|_| "cannot determine private environment policy owner")?
        .uid();
    let parent = path
        .parent()
        .ok_or("private environment policy parent absent")?;
    let parent_information = std::fs::symlink_metadata(parent)
        .map_err(|_| "cannot inspect private environment policy parent")?;
    if !information.is_file()
        || information.nlink() != 1
        || information.uid() != owner
        || information.mode() & 0o077 != 0
        || information.len() > MAX_POLICY_BYTES
        || !parent_information.is_dir()
        || parent_information.uid() != owner
        || parent_information.mode() & 0o022 != 0
    {
        return Err(
            "private environment policy must be owner-private in an owner-controlled directory"
                .into(),
        );
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "cannot open private environment policy without following links")?;
    let opened = file
        .metadata()
        .map_err(|_| "cannot inspect opened private environment policy")?;
    if opened.dev() != information.dev()
        || opened.ino() != information.ino()
        || !opened.is_file()
        || opened.nlink() != 1
        || opened.uid() != owner
        || opened.mode() & 0o077 != 0
    {
        return Err("private environment policy changed while opening".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_POLICY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read private environment policy")?;
    let after = file
        .metadata()
        .map_err(|_| "cannot reinspect private environment policy")?;
    let named = std::fs::symlink_metadata(path)
        .map_err(|_| "cannot reinspect private environment policy name")?;
    if bytes.len() as u64 > MAX_POLICY_BYTES
        || after.dev() != opened.dev()
        || after.ino() != opened.ino()
        || after.len() != opened.len()
        || after.mtime() != opened.mtime()
        || after.mtime_nsec() != opened.mtime_nsec()
        || after.ctime() != opened.ctime()
        || after.ctime_nsec() != opened.ctime_nsec()
        || after.nlink() != 1
        || named.dev() != opened.dev()
        || named.ino() != opened.ino()
        || named.is_symlink()
        || after.uid() != owner
        || after.mode() & 0o077 != 0
    {
        return Err("private environment policy changed while reading".into());
    }
    Ok(Some(bytes))
}

#[cfg(not(target_os = "linux"))]
fn read_owner_policy(path: &Path) -> Result<Option<Vec<u8>>, String> {
    if path.exists() {
        Err("private environment policy requires a supported owner-private file boundary".into())
    } else {
        Ok(None)
    }
}

pub(crate) fn authorized_names(
    manifest: &PluginManifest,
    manifest_bytes: &[u8],
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(BTreeMap::new());
    };
    let path = Path::new(&home).join(".autonomics/plugin-private-environment.json");
    match read_owner_policy(&path)? {
        Some(bytes) => Policy::parse(&bytes)?.authorize(manifest, manifest_bytes),
        None => Ok(BTreeMap::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> serde_json::Value {
        json!({"schema": 1, "grants": [{"plugin": FAMILY, "kind": KINDS[0],
            "manifest_sha256": hash(b"manifest"), "script_sha256": hash(b"script"),
            "image_reference": format!("ghcr.io/zj-2002/ieugwasr-official@{}", hash(b"image")),
            "names": ["OPENGWAS_JWT"]}]})
    }

    #[test]
    fn policy_accepts_only_the_two_named_author_nodes_and_single_private_name() {
        let mut value = policy();
        assert!(Policy::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
        value["grants"][0]["kind"] = json!(KINDS[1]);
        assert!(Policy::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
        for name in ["OPENGWAS_TOKEN", "OPENALEX_API_KEY", "HOME", ""] {
            value["grants"][0]["names"] = json!([name]);
            assert!(Policy::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }

    #[test]
    fn policy_rejects_unknown_fields_duplicate_grants_and_invalid_identities() {
        for field in ["value", "environment", "allow_all"] {
            let mut value = policy();
            value["grants"][0][field] = json!("forbidden");
            assert!(Policy::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        let mut duplicate = policy();
        duplicate["grants"]
            .as_array_mut()
            .unwrap()
            .push(policy()["grants"][0].clone());
        assert!(Policy::parse(&serde_json::to_vec(&duplicate).unwrap()).is_err());
        for field in [
            "manifest_sha256",
            "script_sha256",
            "image_reference",
            "plugin",
            "kind",
        ] {
            let mut value = policy();
            value["grants"][0][field] = json!("invalid");
            assert!(Policy::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        assert!(Policy::parse(br#"{"schema":1,"schema":1,"grants":[]}"#).is_err());
    }

    #[test]
    fn authorization_binds_every_implementation_identity_and_only_the_selected_kind() {
        let image = format!("ghcr.io/zj-2002/ieugwasr-official@{}", hash(b"image"));
        let mut manifest: PluginManifest = serde_json::from_value(json!({
            "schema_version": 1, "plugin_name": FAMILY, "image": {"reference": image},
            "nodes": [{"kind": KINDS[0], "desc": "toy", "doc": "toy",
                "ports": {"inputs": [], "outputs": []},
                "command": {"interpreter": "sh", "script": "script"}}]
        }))
        .unwrap();
        let policy = Policy::parse(&serde_json::to_vec(&policy()).unwrap()).unwrap();
        assert_eq!(
            policy.authorize(&manifest, b"manifest").unwrap(),
            BTreeMap::from([(KINDS[0].to_string(), vec!["OPENGWAS_JWT".to_string()])])
        );
        assert!(policy.authorize(&manifest, b"changed manifest").is_err());
        manifest.nodes[0].command.script = Some("changed script".into());
        assert!(policy.authorize(&manifest, b"manifest").is_err());
        manifest.nodes[0].command.script = Some("script".into());
        manifest.image.reference = container_runtime::image::ImageReference::parse(&format!(
            "ghcr.io/zj-2002/ieugwasr-official@{}",
            hash(b"changed image")
        ))
        .unwrap();
        assert!(policy.authorize(&manifest, b"manifest").is_err());
        manifest.nodes.clear();
        assert!(policy.authorize(&manifest, b"manifest").is_err());
        manifest.plugin_name = "unrelated-family".into();
        assert!(policy.authorize(&manifest, b"manifest").unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn policy_file_requires_private_owner_permissions_and_rejects_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("policy.json");
        assert!(read_owner_policy(&path).unwrap().is_none());
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_owner_policy(&path).unwrap().unwrap(), b"{}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_owner_policy(&path).is_err());
        let link = temporary.path().join("link.json");
        symlink(&path, &link).unwrap();
        assert!(read_owner_policy(&link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(read_owner_policy(&path).is_err());
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_POLICY_BYTES + 1)
            .unwrap();
        assert!(read_owner_policy(&path).is_err());
    }
}
