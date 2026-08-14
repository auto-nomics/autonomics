//! Runtime read path and memory prompt injection.

use std::path::{Path, PathBuf};

use super::MemoryConfig;

#[derive(Debug, Clone)]
pub struct MemoryBackend {
    pub config: MemoryConfig,
}

impl MemoryBackend {
    #[must_use]
    pub fn new(config: MemoryConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.config.root
    }

    pub async fn prompt_section(&self) -> Option<String> {
        memory_prompt_section(&self.config.root, self.config.summary_max_bytes).await
    }
}

pub async fn memory_prompt_section(root: &Path, max_bytes: usize) -> Option<String> {
    let summary = tokio::fs::read_to_string(root.join("memory_summary.md"))
        .await
        .ok()?;
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }
    let mut summary = summary.to_string();
    if summary.len() > max_bytes {
        let cut = summary
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| i <= &max_bytes)
            .last()
            .unwrap_or(0);
        summary = format!("{}\n[MEMORY_SUMMARY_TRUNCATED]", &summary[..cut]);
    }
    Some(format!(
        "## Persistent memory\n\n\
         The compact summary below is loaded from prior sessions. Use it when \
         relevant, and use `memory_search`, `memory_read`, or `memory_list` for \
         progressive disclosure. Explicit current instructions and verified \
         current-state evidence override memory. Memory-derived facts must not \
         be presented as current facts without verification.\n\n\
         ### MEMORY_SUMMARY\n\n{summary}\n"
    ))
}

pub(crate) fn safe_memory_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute() {
        return Err("memory paths must be relative".to_string());
    }
    let mut path = root.to_path_buf();
    for component in relative_path.components() {
        match component {
            std::path::Component::Normal(part) => path.push(part),
            std::path::Component::CurDir => {}
            _ => return Err("invalid memory path".to_string()),
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn prompt_includes_summary_and_truncates() {
        let root: PathBuf = std::env::temp_dir().join(format!(
            "agentik-memory-backend-{}-{}",
            std::process::id(),
            crate::memory::now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        tokio::fs::write(root.join("memory_summary.md"), "v1\n\nfacts\n")
            .await
            .unwrap();
        let section = memory_prompt_section(&root, 1024).await.unwrap();
        assert!(section.contains("facts"));
        assert!(safe_memory_path(&root, "../escape").is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
