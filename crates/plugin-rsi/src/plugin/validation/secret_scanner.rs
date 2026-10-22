//! Conservative credential scan over plugin workspace files.

use crate::foundation::workspace::PluginWorkspace;

/// Scans all safe workspace files for common credential markers.
pub(crate) struct SecretScanner<'a> {
    workspace: &'a PluginWorkspace,
}

impl<'a> SecretScanner<'a> {
    pub(crate) fn new(workspace: &'a PluginWorkspace) -> Self {
        Self { workspace }
    }

    pub(crate) fn scan(&self) -> Result<(), String> {
        for path in self
            .workspace
            .list_files()
            .map_err(|error| error.to_string())?
        {
            let text = self
                .workspace
                .read_text(&path)
                .map_err(|error| error.to_string())?;
            for marker in ["ghp_", "github_pat_", "AKIA", "BEGIN PRIVATE KEY"] {
                if text.contains(marker) {
                    return Err(format!("possible credential in {path}"));
                }
            }
        }
        Ok(())
    }
}
