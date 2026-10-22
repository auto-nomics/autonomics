//! Atomic TOML persistence helpers.

use std::path::Path;

use serde::Serialize;

use crate::{Error, Result};

pub(crate) fn atomic_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let text = toml::to_string_pretty(value).map_err(|source| Error::SerializeToml {
        path: path.to_path_buf(),
        source,
    })?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text).map_err(|source| Error::WriteFile {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| Error::WriteFile {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}
