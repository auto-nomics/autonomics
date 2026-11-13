use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn content_fingerprint(root: &Path) -> Result<String> {
    fn visit(root: &Path, path: &Path, entries: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == ".git" || (path == root && name == ".rsi") {
                continue;
            }
            let path = entry.path();
            let meta = path.symlink_metadata()?;
            if meta.file_type().is_symlink() {
                return Err(Error::Validation(
                    "symlinks cannot identify plugin content".into(),
                ));
            }
            if meta.is_dir() {
                visit(root, &path, entries)?;
            } else if meta.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map_err(|e| Error::Validation(e.to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let mut bytes = std::fs::read(&path)?;
                if rel == "manifest.toml" {
                    let mut value: toml::Value = toml::from_str(&String::from_utf8_lossy(&bytes))
                        .map_err(|e| Error::Validation(e.to_string()))?;
                    if let Some(nodes) = value.get("nodes").and_then(|v| v.as_array()) {
                        for node in nodes {
                            if node
                                .get("command")
                                .and_then(|v| v.get("script_file"))
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| s.trim_start_matches("./").starts_with(".rsi/"))
                            {
                                return Err(Error::Validation(
                                    "runtime scripts cannot reside in host .rsi metadata".into(),
                                ));
                            }
                        }
                    }
                    if let Some(t) = value.as_table_mut() {
                        for key in ["status", "lifecycle", "installation"] {
                            t.remove(key);
                        }
                    }
                    bytes =
                        serde_json::to_vec(&value).map_err(|e| Error::Validation(e.to_string()))?;
                }
                entries.push((rel, bytes));
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    visit(root, root, &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hash = Sha256::new();
    for (name, bytes) in entries {
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_does_not_hide_script_or_parameter_edits() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(
            t.path().join("manifest.toml"),
            "status='draft'\nplugin_name='demo'\n[lifecycle]\nlatest_report='one'\n",
        )
        .unwrap();
        std::fs::write(t.path().join("script.R"), "print(1)").unwrap();
        let first = content_fingerprint(t.path()).unwrap();
        std::fs::write(
            t.path().join("manifest.toml"),
            "status='approved'\nplugin_name='demo'\n[lifecycle]\nlatest_report='two'\n",
        )
        .unwrap();
        std::fs::create_dir(t.path().join(".rsi")).unwrap();
        std::fs::write(t.path().join(".rsi/report.json"), "{}").unwrap();
        assert_eq!(first, content_fingerprint(t.path()).unwrap());
        std::fs::write(t.path().join("script.R"), "print(2)").unwrap();
        assert_ne!(first, content_fingerprint(t.path()).unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_are_rejected() {
        let t = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/passwd", t.path().join("escape")).unwrap();
        assert!(content_fingerprint(t.path()).is_err());
    }
}
