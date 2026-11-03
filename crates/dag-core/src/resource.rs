//! Linux-first memory sampling for DAG execution.
//!
//! The sampler compares each visible finite cgroup ancestor's own charge and
//! limit, including shared descendant workloads, and selects the highest
//! working-set ratio. Subtracting `inactive_file` is a reclaimability
//! approximation, not a guarantee that the kernel can satisfy an allocation.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Configuration for the run-level memory guard.
#[derive(Debug, Clone, Copy)]
pub struct MemoryGuardConfig {
    /// Stop a DAG when sampled memory reaches this fraction of its limit.
    pub threshold_ratio: f64,
    /// Interval between samples.
    pub sample_interval: Duration,
}

impl MemoryGuardConfig {
    pub fn new(threshold_ratio: f64, sample_interval: Duration) -> Option<Self> {
        (threshold_ratio.is_finite() && threshold_ratio > 0.0 && threshold_ratio <= 1.0).then_some(
            Self {
                threshold_ratio,
                sample_interval,
            },
        )
    }
}

/// One memory observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemorySample {
    /// Working-set approximation used for the threshold decision.
    pub usage_bytes: u64,
    /// Raw memory charged to the cgroup (before inactive page-cache removal).
    pub charged_bytes: u64,
    pub limit_bytes: u64,
    pub source: &'static str,
}

impl MemorySample {
    pub fn ratio(&self) -> f64 {
        if self.limit_bytes == 0 {
            0.0
        } else {
            self.usage_bytes as f64 / self.limit_bytes as f64
        }
    }
}

/// Message emitted by the background sampler.
#[derive(Debug)]
pub enum MemoryObservation {
    Sample(MemorySample),
    Unavailable(String),
}

/// Read one memory sample from cgroup v2 or `/proc/meminfo`.
pub fn sample_memory_usage() -> Result<MemorySample, String> {
    if let Some(sample) = sample_cgroup_v2()? {
        return Ok(sample);
    }
    sample_system_memory().ok_or_else(|| "no supported memory source is available".to_string())
}

fn sample_cgroup_v2() -> Result<Option<MemorySample>, String> {
    let contents = read_cgroup_membership()?;
    sample_cgroup_v2_at(Path::new("/sys/fs/cgroup"), &contents)
}

/// Minimum finite capacity across visible cgroup ancestors, independent of pressure.
pub fn effective_memory_limit_bytes() -> Result<Option<u64>, String> {
    let contents = read_cgroup_membership()?;
    effective_memory_limit_bytes_at(Path::new("/sys/fs/cgroup"), &contents)
}

/// Visible cgroup capacity, or checked system capacity when no finite limit exists.
pub fn memory_capacity_bytes() -> Result<u64, String> {
    let contents = read_cgroup_membership()?;
    if let Some(limit) = effective_memory_limit_bytes_at(Path::new("/sys/fs/cgroup"), &contents)? {
        return Ok(limit);
    }
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .map_err(|error| format!("read /proc/meminfo: {error}"))?;
    system_memory_capacity(&meminfo)
}

#[cfg(test)]
fn memory_capacity_bytes_at(root: &Path, contents: &str, meminfo: &str) -> Result<u64, String> {
    effective_memory_limit_bytes_at(root, contents)?
        .map_or_else(|| system_memory_capacity(meminfo), Ok)
}

fn system_memory_capacity(contents: &str) -> Result<u64, String> {
    let mut matches = contents.lines().filter_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next() == Some("MemTotal:")).then(|| parts.collect::<Vec<_>>())
    });
    let fields = matches.next().ok_or("MemTotal not found")?;
    if fields.len() != 2 || fields[1] != "kB" || matches.next().is_some() {
        return Err("one MemTotal value in kB required".into());
    }
    let total = fields[0]
        .parse::<u64>()
        .map_err(|error| format!("parse MemTotal: {error}"))?
        .checked_mul(1024)
        .filter(|total| *total > 0)
        .ok_or("positive representable memory capacity required")?;
    Ok(total)
}

fn read_cgroup_membership() -> Result<String, String> {
    std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|error| format!("read /proc/self/cgroup: {error}"))
}

fn cgroup_directories(root: &Path, contents: &str) -> Result<Option<Vec<PathBuf>>, String> {
    let mut unified = None;
    if contents.lines().next().is_none() {
        return Err("empty cgroup membership".into());
    }
    for line in contents.lines() {
        let mut parts = line.splitn(3, ':');
        let hierarchy = parts.next().ok_or("missing cgroup hierarchy")?;
        let controllers = parts.next().ok_or("missing cgroup controllers")?;
        let membership = parts.next().ok_or("missing cgroup path")?;
        let hierarchy_id = hierarchy
            .parse::<u64>()
            .map_err(|error| format!("parse cgroup hierarchy: {error}"))?;
        if hierarchy_id == 0 {
            if hierarchy != "0" {
                return Err("canonical unified hierarchy identifier required".into());
            }
            if !controllers.is_empty() || unified.is_some() {
                return Err("one unified cgroup membership required".into());
            }
            unified = Some(membership);
        }
    }
    let Some(membership) = unified else {
        return Ok(None);
    };
    if !membership.starts_with('/')
        || membership.contains('\0')
        || (membership != "/"
            && membership[1..]
                .split('/')
                .any(|part| matches!(part, "" | "." | "..")))
    {
        return Err("canonical absolute unified cgroup path required".into());
    }
    if !root.is_absolute() {
        return Err("absolute visible cgroup root required".into());
    }
    let current = root.join(membership.trim_start_matches('/'));
    let metadata = std::fs::metadata(&current)
        .map_err(|error| format!("read cgroup directory {}: {error}", current.display()))?;
    if !metadata.is_dir() {
        return Err("current cgroup must be a directory".into());
    }
    let mut directories = Vec::new();
    let mut cursor = current.as_path();
    loop {
        directories.push(cursor.to_path_buf());
        if cursor == root {
            break;
        }
        cursor = cursor.parent().ok_or("cgroup path escaped visible root")?;
        if !cursor.starts_with(root) {
            return Err("cgroup path escaped visible root".into());
        }
    }
    Ok(Some(directories))
}

fn sample_cgroup_v2_at(root: &Path, contents: &str) -> Result<Option<MemorySample>, String> {
    let Some(directories) = cgroup_directories(root, contents)? else {
        return Ok(None);
    };
    let mut selected: Option<MemorySample> = None;
    for directory in directories {
        let current = read_optional_u64(&directory.join("memory.current"))?;
        let limit_contents = read_optional_text(&directory.join("memory.max"))?;
        let limit = limit_contents
            .as_deref()
            .map(parse_optional_limit)
            .transpose()
            .map_err(|error| format!("parse {}/memory.max: {error}", directory.display()))?
            .flatten();
        if current.is_some() != limit_contents.is_some() {
            if directory == root && current.is_some() && limit_contents.is_none() {
                continue;
            }
            return Err(format!(
                "incomplete memory controller at {}",
                directory.display()
            ));
        }
        let Some(charged_bytes) = current else {
            continue;
        };
        let Some(limit_bytes) = limit else {
            continue;
        };
        let inactive_file = match read_optional_text(&directory.join("memory.stat"))? {
            Some(contents) => parse_stat_u64(&contents, "inactive_file")
                .map_err(|error| format!("parse {}/memory.stat: {error}", directory.display()))?,
            None => 0,
        };
        let candidate = MemorySample {
            usage_bytes: charged_bytes.saturating_sub(inactive_file),
            charged_bytes,
            limit_bytes,
            source: "cgroup_v2",
        };
        if selected.is_none_or(|previous| {
            u128::from(candidate.usage_bytes) * u128::from(previous.limit_bytes)
                > u128::from(previous.usage_bytes) * u128::from(candidate.limit_bytes)
        }) {
            selected = Some(candidate);
        }
    }
    Ok(selected)
}

fn effective_memory_limit_bytes_at(root: &Path, contents: &str) -> Result<Option<u64>, String> {
    let Some(directories) = cgroup_directories(root, contents)? else {
        return Ok(None);
    };
    let mut minimum: Option<u64> = None;
    for directory in directories {
        if let Some(limit) = read_optional_limit(&directory.join("memory.max"))? {
            minimum = Some(minimum.map_or(limit, |previous| previous.min(limit)));
        }
    }
    Ok(minimum)
}

fn read_optional_text(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn read_optional_limit(path: &Path) -> Result<Option<u64>, String> {
    let Some(contents) = read_optional_text(path)? else {
        return Ok(None);
    };
    parse_optional_limit(&contents).map_err(|error| format!("parse {}: {error}", path.display()))
}

fn parse_optional_limit(contents: &str) -> Result<Option<u64>, String> {
    let value = contents.trim();
    if value == "max" {
        return Ok(None);
    }
    let limit = value
        .parse::<u64>()
        .map_err(|error| format!("parse memory limit: {error}"))?;
    if limit == 0 {
        return Err("zero memory limit".into());
    }
    Ok(Some(limit))
}

fn read_optional_u64(path: &Path) -> Result<Option<u64>, String> {
    let Some(value) = read_optional_text(path)? else {
        return Ok(None);
    };
    value
        .trim()
        .parse::<u64>()
        .map(Some)
        .map_err(|error| format!("parse integer from {}: {error}", path.display()))
}

fn parse_stat_u64(contents: &str, key: &str) -> Result<u64, String> {
    let mut matches = contents.lines().filter_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next() == Some(key)).then(|| parts.collect::<Vec<_>>())
    });
    let values = matches
        .next()
        .ok_or_else(|| format!("key `{key}` not found"))?;
    if values.len() != 1 || matches.next().is_some() {
        return Err(format!("one integer value required for key `{key}`"));
    }
    values[0]
        .parse::<u64>()
        .map_err(|error| format!("parse key `{key}`: {error}"))
}

fn sample_system_memory() -> Option<MemorySample> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total_kb = None;
    let mut available_kb = None;
    for line in contents.lines() {
        let mut parts = line.split_whitespace();
        match parts.next()? {
            "MemTotal:" => total_kb = parts.next()?.parse::<u64>().ok(),
            "MemAvailable:" => available_kb = parts.next()?.parse::<u64>().ok(),
            _ => {}
        }
    }
    let total_kb = total_kb?;
    let available_kb = available_kb.unwrap_or(0);
    let total = total_kb.saturating_mul(1024);
    let usage = total.saturating_sub(available_kb.saturating_mul(1024));
    Some(MemorySample {
        usage_bytes: usage,
        charged_bytes: usage,
        limit_bytes: total,
        source: "system",
    })
}

#[cfg(test)]
#[path = "resource_tests.rs"]
mod resource_tests;
