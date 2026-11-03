//! Per-invocation output destinations, independent of the immutable node spec.
//! This is a physical execution binding; it grants no scientific authorization.
use crate::dag::DagError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
}

#[cfg(unix)]
fn directory_identity(path: &Path) -> Result<DirectoryIdentity, DagError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(|_| refused("private directory missing"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.mode() & 0o777 != 0o700 {
        return Err(refused("real mode0700 directory required"));
    }
    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        uid: metadata.uid(),
        gid: metadata.gid(),
        mode: metadata.mode(),
    })
}

#[cfg(not(unix))]
fn directory_identity(_: &Path) -> Result<DirectoryIdentity, DagError> {
    Err(refused("private directory identity requires Unix"))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOutputProjection {
    run_id: String,
    staging_root: PathBuf,
    destinations: BTreeMap<PathBuf, PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staging_identity: Option<DirectoryIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_root_identity: Option<DirectoryIdentity>,
}

fn refused(message: &str) -> DagError {
    DagError::Schedule(format!("runtime output projection: {message}"))
}

fn absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        && path
            .to_str()
            .is_some_and(|s| !s.chars().any(char::is_control))
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
            }
        })
}

impl RuntimeOutputProjection {
    /// Targets must be new directories under staging_root/<engine run UUID>/.
    /// This constructor does not create directories or alter a DAG definition.
    pub fn new(
        run_id: String,
        staging_root: PathBuf,
        destinations: BTreeMap<PathBuf, PathBuf>,
    ) -> Result<Self, DagError> {
        let value = Self {
            run_id,
            staging_root,
            destinations,
            staging_identity: None,
            run_root_identity: None,
        };
        value.validate_shape()?;
        Ok(value)
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }
    pub fn destinations(&self) -> &BTreeMap<PathBuf, PathBuf> {
        &self.destinations
    }

    pub fn validate_shape(&self) -> Result<(), DagError> {
        if !canonical_uuid(&self.run_id)
            || !absolute(&self.staging_root)
            || self.staging_root.parent().is_none()
            || self.destinations.is_empty()
            || self.destinations.len() > 4096
        {
            return Err(refused("bounded canonical UUID/root/destinations required"));
        }
        let run_root = self.staging_root.join(&self.run_id);
        let physical: Vec<_> = self.destinations.values().collect();
        for (logical, target) in &self.destinations {
            if !absolute(logical)
                || !absolute(target)
                || logical.parent().is_none()
                || !target
                    .strip_prefix(&run_root)
                    .is_ok_and(|relative| !relative.as_os_str().is_empty())
                || self
                    .destinations
                    .keys()
                    .any(|other| other.starts_with(target) || target.starts_with(other))
            {
                return Err(refused(
                    "logical/physical paths must be separate run-scoped directories",
                ));
            }
        }
        for (i, path) in physical.iter().enumerate() {
            if physical
                .iter()
                .skip(i + 1)
                .any(|other| path.starts_with(other) || other.starts_with(path))
            {
                return Err(refused("physical destinations overlap"));
            }
        }
        Ok(())
    }

    /// Checked against actual factory declarations and path reads before dispatch.
    /// The exact declaration set prevents silent unmapped or extra publishers.
    pub fn validate_for_run(
        &self,
        run_id: &str,
        declared: &BTreeSet<PathBuf>,
        inputs: &[PathBuf],
    ) -> Result<(), DagError> {
        self.validate_shape()?;
        if self.run_id != run_id
            || self.destinations.keys().cloned().collect::<BTreeSet<_>>() != *declared
        {
            return Err(refused(
                "actual run UUID or complete publisher declaration set differs",
            ));
        }
        for (logical, target) in &self.destinations {
            if inputs
                .iter()
                .any(|path| path.starts_with(logical) || path.starts_with(target))
            {
                return Err(refused("publisher aliases a configured input"));
            }
        }
        Ok(())
    }

    pub fn resolve(&self, logical: &Path) -> Result<PathBuf, DagError> {
        self.destinations
            .get(logical)
            .cloned()
            .ok_or_else(|| refused("publisher has no declared physical destination"))
    }

    /// Establish a new physical run boundary before recording the final intent.
    /// Existing or partially initialized directories are preserved and refused.
    pub fn initialize_run_directory(mut self) -> Result<Self, DagError> {
        self.validate_shape()?;
        if self.staging_identity.is_some() || self.run_root_identity.is_some() {
            return Err(refused(
                "only an uninitialized projection may establish a run directory",
            ));
        }
        self.verify_parent_chain()?;
        let parent = directory_identity(&self.staging_root)?;
        let run_root = self.staging_root.join(&self.run_id);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&run_root)
                .map_err(|_| refused("run directory must be newly created"))?;
        }
        #[cfg(not(unix))]
        return Err(refused("private run directories require Unix"));
        let child = directory_identity(&run_root)?;
        if directory_identity(&self.staging_root)? != parent
            || child.uid != parent.uid
            || child.gid != parent.gid
        {
            return Err(refused(
                "run/staging identity changed during initialization",
            ));
        }
        self.staging_identity = Some(parent);
        self.run_root_identity = Some(child);
        self.verify_runtime_directory()?;
        Ok(self)
    }

    fn verify_parent_chain(&self) -> Result<(), DagError> {
        for path in self.staging_root.ancestors() {
            let info = fs::symlink_metadata(path).map_err(|_| refused("staging parent missing"))?;
            if !info.is_dir() || info.file_type().is_symlink() {
                return Err(refused("staging ancestry contains an alias"));
            }
        }
        Ok(())
    }

    /// Rechecked by each publisher; no mutable environment mapping is consulted.
    pub fn verify_runtime_directory(&self) -> Result<(), DagError> {
        self.validate_shape()?;
        self.verify_parent_chain()?;
        if self.staging_identity.as_ref() != Some(&directory_identity(&self.staging_root)?)
            || self.run_root_identity.as_ref()
                != Some(&directory_identity(&self.staging_root.join(&self.run_id))?)
        {
            return Err(refused("actual initialized run directory identity differs"));
        }
        Ok(())
    }

    /// BLAKE3 of this typed serialization, matching the engine's run-start field.
    /// Artifact-store identities remain independently computed SHA256.
    pub fn identity_blake3(&self) -> Result<String, DagError> {
        let raw =
            serde_json::to_vec(self).map_err(|_| refused("projection serialization failed"))?;
        Ok(blake3::hash(&raw).to_hex().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const RUN: &str = "ba4fbb5f-c9c1-4a2a-9a11-1bfe0377d5a1";
    fn projection() -> RuntimeOutputProjection {
        RuntimeOutputProjection::new(
            RUN.into(),
            "/private/staging".into(),
            BTreeMap::from([
                (
                    "/frozen/requests/one".into(),
                    Path::new("/private/staging").join(RUN).join("one"),
                ),
                (
                    "/frozen/requests/two".into(),
                    Path::new("/private/staging").join(RUN).join("two"),
                ),
            ]),
        )
        .unwrap()
    }
    #[test]
    fn exact_uuid_complete_declarations_and_source_aliases_are_checked() {
        let value = projection();
        let declared = value.destinations.keys().cloned().collect();
        value
            .validate_for_run(RUN, &declared, &["/inputs/data.tsv".into()])
            .unwrap();
        assert!(
            value
                .validate_for_run("b0000000-0000-0000-0000-000000000000", &declared, &[])
                .is_err()
        );
        assert!(value.validate_for_run(RUN, &BTreeSet::new(), &[]).is_err());
        for path in [
            PathBuf::from("/frozen/requests/one/input.tsv"),
            value
                .destinations
                .values()
                .next()
                .unwrap()
                .join("input.tsv"),
        ] {
            assert!(value.validate_for_run(RUN, &declared, &[path]).is_err());
        }
        assert!(value.resolve(Path::new("/undeclared")).is_err());
    }
    #[test]
    fn physical_escapes_aliases_and_overlapping_publishers_are_rejected() {
        let value = projection();
        for target in [
            PathBuf::from("/outside"),
            Path::new("/private/staging").join(RUN),
            PathBuf::from("/frozen/requests/one"),
            Path::new("/private/staging").join(RUN).join("../escape"),
        ] {
            let mut changed = value.clone();
            *changed.destinations.values_mut().next().unwrap() = target;
            assert!(changed.validate_shape().is_err());
        }
        let mut changed = value.clone();
        let target = changed.destinations.values().next().unwrap().clone();
        *changed.destinations.values_mut().last().unwrap() = target.join("child");
        assert!(changed.validate_shape().is_err());
    }
    #[test]
    fn projection_roundtrip_does_not_change_logical_paths() {
        let value = projection();
        let raw = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            serde_json::from_slice::<RuntimeOutputProjection>(&raw).unwrap(),
            value
        );
        assert_eq!(
            value.resolve(Path::new("/frozen/requests/one")).unwrap(),
            Path::new("/private/staging").join(RUN).join("one")
        );
    }
    #[test]
    fn cross_row_alias_and_noncanonical_path_spellings_are_rejected() {
        let mut value = projection();
        let target = value.destinations.values().next().unwrap().clone();
        value.destinations.insert(
            target.join("child"),
            Path::new("/private/staging").join(RUN).join("three"),
        );
        assert!(value.validate_shape().is_err());
        for logical in ["/frozen//one", "/frozen/./one"] {
            assert!(
                RuntimeOutputProjection::new(
                    RUN.into(),
                    "/private/staging".into(),
                    BTreeMap::from([(
                        logical.into(),
                        Path::new("/private/staging").join(RUN).join("one")
                    )])
                )
                .is_err()
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn fresh_directory_identity_is_rechecked_and_prior_run_is_not_adopted() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let draft = RuntimeOutputProjection::new(
            RUN.into(),
            temporary.path().to_path_buf(),
            BTreeMap::from([("/frozen/one".into(), temporary.path().join(RUN).join("one"))]),
        )
        .unwrap();
        assert!(draft.verify_runtime_directory().is_err());
        let actual = draft.clone().initialize_run_directory().unwrap();
        actual.verify_runtime_directory().unwrap();
        assert!(draft.initialize_run_directory().is_err());
        let path = temporary.path().join(RUN);
        fs::rename(&path, temporary.path().join("prior")).unwrap();
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(actual.verify_runtime_directory().is_err());
    }
}
