use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::error::ContainerRuntimeError;
use crate::types::{ContainerRunRequest, ContainerRunResult};

pub const DEFAULT_CONTAINER_WORKDIR: &str = "/work";
pub const DEFAULT_TIMEOUT_SECS: u64 = 3600;
pub const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;

/// Runtime-neutral execution of one ephemeral container.
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    async fn run(
        &self,
        request: ContainerRunRequest,
    ) -> Result<ContainerRunResult, ContainerRuntimeError>;

    fn name(&self) -> &'static str {
        "container"
    }

    /// Root of the shared workspace volume as seen by this control process.
    fn workspace_root(&self) -> &Path {
        Path::new("/")
    }
}

pub type SharedContainerRuntime = Arc<dyn ContainerRuntime>;

pub fn unique_container_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "autonomics-container-command-{}-{nanos}",
        std::process::id()
    )
}
