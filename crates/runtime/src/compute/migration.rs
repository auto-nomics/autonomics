//! One-time compatibility for the former versioned compute registry.
use super::*;

/// Keep durable job intents, monitor outboxes and host-absolute evidence paths
/// accessible after collapsing the target history. Original files stay intact.
pub(super) fn restore_execution_state(
    registry: &ComputeRegistry,
    bytes: &[u8],
) -> Result<(), String> {
    let legacy: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let mut copies = Vec::new();
    for value in legacy["targets"]
        .as_array()
        .ok_or("invalid legacy compute targets")?
    {
        let mut value = value.clone();
        let revision = value
            .as_object_mut()
            .ok_or("invalid legacy target")?
            .remove("revision")
            .and_then(|v| v.as_u64())
            .ok_or("invalid legacy target revision")?;
        let old: ComputeTarget = serde_json::from_value(value).map_err(|e| e.to_string())?;
        identifier(&old.id)?;
        let current = registry
            .state()
            .targets
            .into_iter()
            .find(|t| t.id == old.id)
            .ok_or("legacy target disappeared during migration")?;
        let root = registry
            .root
            .join("targets")
            .join(format!("{}-r{revision}", old.id));
        let destination = if old.id == "legacy-slurm" {
            registry.config.state_dir.join("hpc-account")
        } else {
            let target_root = registry.root.join("targets").join(&old.id);
            private_dir(&target_root)?;
            target_root.join("hpc-account")
        };
        let account = if old.id == "legacy-slurm" && revision == 1 {
            destination.clone()
        } else {
            root.join("hpc-account")
        };
        if !account.exists() {
            continue;
        }
        if !same_job_connection(&old, &current) {
            let monitors = account.join("job-monitors");
            if monitors.is_dir() {
                for entry in fs::read_dir(&monitors).map_err(|e| e.to_string())? {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    if path.extension().is_none_or(|v| v != "json") {
                        continue;
                    }
                    let record: hpc_tools::monitor::MonitorRecord =
                        serde_json::from_slice(&read_private(&path)?).map_err(|e| e.to_string())?;
                    if record.view().completion_pending() {
                        return Err(format!(
                            "target {} has unfinished jobs on a different legacy SSH connection; resolve those jobs before migrating",
                            old.id
                        ));
                    }
                }
            }
        }
        if account != destination {
            copies.push((account, destination));
        }
    }
    // Validate every original account before copying into the seed's shared
    // directory, so newly copied records cannot be mistaken for older state.
    for (source, destination) in copies {
        copy_records(&source, &destination, 0)?;
    }
    Ok(())
}

fn copy_records(source: &Path, destination: &Path, depth: usize) -> Result<(), String> {
    if depth > 4 {
        return Err("unexpected nesting in legacy job state".into());
    }
    private_dir(source)?;
    private_dir(destination)?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().starts_with(".pending-") {
            continue;
        }
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
        if metadata.is_dir() {
            copy_records(&from, &to, depth + 1)?;
            continue;
        }
        let bytes = read_private(&from)?;
        if fs::symlink_metadata(&to).is_ok() {
            if read_private(&to)? != bytes {
                // Never overwrite an uncertain original submission with another
                // intent just because its target used to have a larger counter.
                return Err("conflicting legacy job records; reconcile the original submissions before migrating".into());
            }
        } else {
            write_bytes_new(&to, &bytes)?;
        }
    }
    Ok(())
}

/// Old receipts contain absolute paths. Preserve read-only access to their
/// dedicated evidence directories on every startup, without loading old targets.
pub(super) fn mount_legacy_evidence(registry: &ComputeRegistry) -> Result<(), String> {
    let state = registry.state();
    for entry in fs::read_dir(registry.root.join("targets")).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some((id, suffix)) = name.to_str().and_then(|v| v.rsplit_once("-r")) else {
            continue;
        };
        if suffix.parse::<u64>().is_err() || !state.targets.iter().any(|t| t.id == id) {
            continue;
        }
        private_dir(&entry.path())?;
        for name in ["tasks", "artifacts", "containers"] {
            let evidence = entry.path().join(name);
            if fs::symlink_metadata(&evidence).is_ok() {
                registry.mount_execution_evidence(&evidence)?;
            }
        }
    }
    Ok(())
}
