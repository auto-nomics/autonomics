//! Cross-run instance result cache — the `-resume` of the flow engine.
//!
//! The cache stores, per process and item key, the fingerprint that produced
//! an instance's outputs plus the **file-valued** outputs themselves. On a
//! `resume` run, an instance whose recomputed fingerprint matches and whose
//! cached files still exist (size-checked through the VFS) is skipped
//! entirely and its outputs are re-published to downstream instances.
//!
//! # Scope decisions (Nextflow correspondences)
//!
//! - **File-backed outputs only.** DataFrame-valued ports are process-local
//!   memory and cannot survive a run; instances producing them simply miss
//!   and re-execute. dag-core's engine-level result cache makes the same
//!   trade-off ("DataFrame results intentionally remain agent-local").
//!   Nextflow caches the task work directory, so *its* outputs are files by
//!   construction — our data-model is richer, hence the filter.
//! - **Validation is cheap existence + size**, not content hashing
//!   (Nextflow's STANDARD `HashMode` hashes file metadata, `deep` hashes
//!   content; we start at the metadata level — a `deep` mode can reuse
//!   dag-core's `file_sha256` later).
//! - The cache is JSON-serializable so a later step can persist it next to
//!   the engine state; in-memory it already covers the dominant
//!   within-session "second batch adds 10 files" scenario.

use std::collections::BTreeMap;

use dag_core::dag::PortOutputs;
use dag_core::value::{FileRef, NodeValue};
use vfs::OpendalFileStorage;

use crate::workflow::instance::{InstanceId, UpstreamBinding, instance_fingerprint_of};
use crate::workflow::plan::ExecutionPlan;

/// One cached output port. Equality is structural on paths (FileRef does
/// not implement `PartialEq`), which is what cache tests need to assert.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CachedOutput {
    File(FileRef),
    Files(Vec<FileRef>),
}

impl CachedOutput {
    fn as_node_value(&self) -> Option<NodeValue> {
        match self {
            CachedOutput::File(file) => Some(NodeValue::File(file.clone())),
            CachedOutput::Files(files) if !files.is_empty() => {
                Some(NodeValue::FileSet(files.clone()))
            }
            CachedOutput::Files(_) => None,
        }
    }
}

/// The cached result of one instance.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedInstance {
    pub fingerprint: String,
    /// File-backed outputs keyed by port. Ports that carried DataFrame or
    /// Channel values are absent — their absence makes the instance a miss.
    pub outputs: BTreeMap<u8, CachedOutput>,
}

impl CachedInstance {
    /// Rebuild the port outputs for a reuse. Returns `None` when no cached
    /// port is representable (defensive: record never stores such entries).
    pub fn to_port_outputs(&self) -> Option<PortOutputs> {
        let mut outputs = PortOutputs::new();
        let mut any = false;
        for (port, cached) in &self.outputs {
            if let Some(value) = cached.as_node_value() {
                outputs.insert(*port, value);
                any = true;
            }
        }
        any.then_some(outputs)
    }

    /// Cheap validity: every cached file must still exist with the size the
    /// fingerprint recorded. VFS paths require the storage view; local paths
    /// stat directly. Any doubt is a miss — reuse never resurrects a
    /// dangling reference.
    pub async fn still_valid(&self, storage: Option<&OpendalFileStorage>) -> bool {
        let files = self.outputs.values().flat_map(|cached| match cached {
            CachedOutput::File(file) => vec![file.clone()],
            CachedOutput::Files(files) => files.clone(),
        });
        for file in files {
            let expected = file.fingerprint.as_ref().map(|fp| fp.size);
            if let Some(virtual_path) = file.path.strip_prefix("vfs://") {
                let Some(storage) = storage else {
                    return false;
                };
                match storage.content_length(virtual_path).await {
                    Ok(size) if expected.is_none_or(|expected| size == expected) => {}
                    _ => return false,
                }
            } else {
                let path = file.path.strip_prefix("file://").unwrap_or(&file.path);
                match std::fs::metadata(path) {
                    Ok(meta) if expected.is_none_or(|expected| meta.len() == expected) => {}
                    _ => return false,
                }
            }
        }
        true
    }
}

/// Process-major cache: `{process → {item key → cached instance}}`. Singleton
/// instances live under the key `"0"`, matching [`InstanceId`] conventions.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstanceCache {
    entries: BTreeMap<String, BTreeMap<String, CachedInstance>>,
}

impl InstanceCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.values().map(BTreeMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn lookup(&self, process: &str, item_key: Option<&str>) -> Option<&CachedInstance> {
        self.entries
            .get(process)
            .and_then(|keys| keys.get(item_key.unwrap_or("0")))
    }

    /// Record one instance's file-backed outputs under `fingerprint`.
    /// Ports carrying non-file values are dropped; an instance whose outputs
    /// are entirely non-file stores an empty entry (which always misses —
    /// visible, honest, and cheap).
    pub fn record(
        &mut self,
        process: &str,
        item_key: Option<&str>,
        fingerprint: String,
        outputs: &PortOutputs,
    ) {
        let mut cached = BTreeMap::new();
        for (port, value) in outputs.iter() {
            match value {
                NodeValue::File(file) => {
                    cached.insert(*port, CachedOutput::File(file.clone()));
                }
                NodeValue::FileSet(files) if !files.is_empty() => {
                    cached.insert(*port, CachedOutput::Files(files.clone()));
                }
                _ => {}
            }
        }
        self.entries.entry(process.to_string()).or_default().insert(
            item_key.unwrap_or("0").to_string(),
            CachedInstance {
                fingerprint,
                outputs: cached,
            },
        );
    }

    /// Drop one instance's entry — used when a re-execution fails so a stale
    /// success can never be served later.
    pub fn invalidate(&mut self, process: &str, item_key: Option<&str>) {
        if let Some(keys) = self.entries.get_mut(process) {
            keys.remove(item_key.unwrap_or("0"));
        }
    }

    /// Serialize for persistence (round-trips through [`Self::from_json`]).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    pub fn from_json(value: serde_json::Value) -> Option<Self> {
        serde_json::from_value(value).ok()
    }
}

/// Compute the fingerprint of one instance from plan state. Upstream
/// bindings derive from the plan's routes; `effective` carries the
/// fingerprint of every upstream instance (reused or freshly executed) in
/// dependency order, which the scheduler guarantees has been settled.
pub fn plan_fingerprint(
    plan: &ExecutionPlan,
    id: &InstanceId,
    effective: &BTreeMap<InstanceId, String>,
) -> Option<String> {
    let (process, spec) = plan
        .processes
        .values()
        .find(|process_plan| process_plan.instances.iter().any(|i| &i.id == id))
        .map(|process_plan| (process_plan.process.clone(), process_plan.spec.clone()))?;
    let instance = plan
        .processes
        .get(&process)
        .and_then(|process_plan| process_plan.instances.iter().find(|i| &i.id == id))?
        .clone();
    let mut upstream = Vec::new();
    for route in plan.routes_to(id) {
        let fingerprint = effective.get(&route.from).cloned()?;
        upstream.push(UpstreamBinding {
            // Same derivation as the executor submission's input binding
            // names — renaming an input invalidates dependents, matching
            // Nextflow hashing input names.
            name: format!("port_{}", route.to_port),
            fingerprint,
        });
    }
    Some(instance_fingerprint_of(
        &process,
        &spec,
        &instance,
        &upstream,
        dag_core::engine_version(),
        dag_core::source_revision(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> FileRef {
        FileRef {
            domain: Default::default(),
            path: path.into(),
            format: None,
            fingerprint: Some(dag_core::value::FileFingerprint {
                size,
                mtime_ns: 1,
                content_hash: None,
                immutable_remote: false,
            }),
        }
    }

    fn outputs_with_file(port: u8, path: &str) -> PortOutputs {
        let mut outputs = PortOutputs::new();
        outputs.insert_file(port, file(path, 7));
        outputs
    }

    #[test]
    fn record_and_lookup_round_trip_through_json() {
        let mut cache = InstanceCache::new();
        let outputs = outputs_with_file(0, "vfs://data/a.out");
        cache.record("qc", Some("a"), "fp-1".into(), &outputs);
        assert_eq!(cache.len(), 1);

        let restored = InstanceCache::from_json(cache.to_json()).unwrap();
        let cached = restored.lookup("qc", Some("a")).unwrap();
        assert_eq!(cached.fingerprint, "fp-1");
        match cached.outputs.get(&0) {
            Some(CachedOutput::File(cached_file)) => {
                assert_eq!(cached_file.path, "vfs://data/a.out");
                assert_eq!(cached_file.fingerprint.as_ref().unwrap().size, 7);
            }
            other => panic!("expected a cached file, got {other:?}"),
        }
        assert!(cached.to_port_outputs().unwrap().get(&0).is_some());
    }

    #[test]
    fn singletons_live_under_the_zero_key() {
        let mut cache = InstanceCache::new();
        cache.record(
            "multiqc",
            None,
            "fp".into(),
            &outputs_with_file(0, "vfs://m"),
        );
        assert!(cache.lookup("multiqc", None).is_some());
        assert!(cache.lookup("multiqc", Some("0")).is_some());
    }

    #[test]
    fn invalidate_removes_only_the_named_instance() {
        let mut cache = InstanceCache::new();
        cache.record("qc", Some("a"), "fp-a".into(), &outputs_with_file(0, "a"));
        cache.record("qc", Some("b"), "fp-b".into(), &outputs_with_file(0, "b"));
        cache.invalidate("qc", Some("a"));
        assert!(cache.lookup("qc", Some("a")).is_none());
        assert!(cache.lookup("qc", Some("b")).is_some());
    }

    #[test]
    fn dataframe_only_outputs_store_an_empty_entry() {
        let mut outputs = PortOutputs::new();
        outputs.insert(0, NodeValue::File(file("x", 1)));
        // A DataFrame port is not representable; simulating via FileSet empty
        // (also unrepresentable) plus one file port.
        outputs.insert(1, NodeValue::FileSet(vec![]));
        let mut cache = InstanceCache::new();
        cache.record("p", Some("k"), "fp".into(), &outputs);
        let cached = cache.lookup("p", Some("k")).unwrap();
        assert_eq!(cached.outputs.len(), 1);
        assert!(cached.outputs.contains_key(&0));
    }

    #[tokio::test]
    async fn local_validation_checks_existence_and_size() {
        let directory = tempfile_dir();
        let real = directory.path().join("a.out");
        std::fs::write(&real, b"1234567").unwrap();
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, FileRef::local(&real, None).unwrap());

        let mut cache = InstanceCache::new();
        cache.record("qc", Some("a"), "fp".into(), &outputs);
        let cached = cache.lookup("qc", Some("a")).unwrap();
        assert!(
            cached.still_valid(None).await,
            "file exists with recorded size"
        );

        std::fs::write(&real, b"12345678").unwrap();
        assert!(
            !cached.still_valid(None).await,
            "size drift must invalidate reuse"
        );

        std::fs::remove_file(&real).unwrap();
        assert!(!cached.still_valid(None).await, "deleted file must miss");

        drop(directory);
    }

    fn tempfile_dir() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    #[tokio::test]
    async fn vfs_paths_require_storage() {
        let mut cache = InstanceCache::new();
        cache.record(
            "qc",
            Some("a"),
            "fp".into(),
            &outputs_with_file(0, "vfs://data/a.out"),
        );
        let cached = cache.lookup("qc", Some("a")).unwrap();
        assert!(
            !cached.still_valid(None).await,
            "a VFS path without a storage view is a miss, not a guess"
        );
    }
}
