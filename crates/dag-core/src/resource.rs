//! Linux-first memory sampling for DAG execution.
//!
//! The sampler prefers the current process's cgroup charge because it includes
//! child workloads such as OCI containers. `inactive_file` is subtracted to
//! approximate the cgroup working set; page cache in that bucket is readily
//! reclaimable and otherwise causes false positives on data-intensive DAGs.

use std::path::PathBuf;
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
    let root = PathBuf::from("/sys/fs/cgroup");
    let current_dir = current_cgroup_dir(&root).unwrap_or_else(|| root.clone());

    for dir in [current_dir, root] {
        let current_path = dir.join("memory.current");
        if !current_path.is_file() {
            continue;
        }
        let charged_bytes = read_u64(&current_path)
            .map_err(|error| format!("read {}: {error}", current_path.display()))?;
        let Some(limit_bytes) = read_optional_limit(&dir.join("memory.max"))? else {
            return Ok(None);
        };
        if limit_bytes == 0 {
            return Ok(None);
        }

        // Prefer working set over raw charge. Missing memory.stat is not fatal;
        // fall back to the conservative raw cgroup charge.
        let stat_path = dir.join("memory.stat");
        let inactive_file = if stat_path.is_file() {
            read_stat_u64(&stat_path, "inactive_file")?
        } else {
            0
        };
        let usage_bytes = charged_bytes.saturating_sub(inactive_file);
        return Ok(Some(MemorySample {
            usage_bytes,
            charged_bytes,
            limit_bytes,
            source: "cgroup_v2",
        }));
    }

    Ok(None)
}

fn current_cgroup_dir(root: &PathBuf) -> Option<PathBuf> {
    let contents = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let relative = contents.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("0"), Some(""), Some(path)) if path != "/" => Some(path.to_string()),
            _ => None,
        }
    })?;
    let path = std::path::Path::new(&relative);
    if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(root.join(path))
}

fn read_optional_limit(path: &PathBuf) -> Result<Option<u64>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let value = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?
        .trim()
        .to_string();
    if value == "max" {
        return Ok(None);
    }
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|error| format!("parse {}: {error}", path.display()))
}

fn read_u64(path: &PathBuf) -> Result<u64, String> {
    let value = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    value
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("parse integer from {}: {error}", path.display()))
}

fn read_stat_u64(path: &PathBuf, key: &str) -> Result<u64, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    contents
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(found), Some(value)) if found == key => Some(value.to_string()),
                _ => None,
            }
        })
        .ok_or_else(|| format!("key `{key}` not found"))?
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
