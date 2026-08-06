mod view_task_results;
mod view_task_status;
mod wait_task;

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::tools::ToolRegistration;
use crate::tools::task_runtime::TaskStore;

pub use view_task_results::{TaskResultViewerTool, ViewTaskResultsInput};
pub use view_task_status::{TaskStatusViewerTool, ViewTaskStatusInput};
pub use wait_task::{WaitTaskInput, WaitTaskTool};

pub fn task_registrations(tasks: Arc<RwLock<TaskStore>>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(TaskResultViewerTool::new(tasks.clone())),
        ToolRegistration::from(TaskStatusViewerTool::new(tasks.clone())),
        ToolRegistration::from(WaitTaskTool::new(tasks)),
    ]
}
