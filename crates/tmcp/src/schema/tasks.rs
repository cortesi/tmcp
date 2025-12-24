use serde::{Deserialize, Serialize};

use super::{NotificationParams, PaginatedResult, Result};
use crate::macros::with_meta;

/// The status of a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Working,
    InputRequired,
    Completed,
    Failed,
    Cancelled,
}

/// Metadata for augmenting a request with task execution.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskMetadata {
    /// Requested duration in milliseconds to retain task from creation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u64>,
}

/// Metadata for associating messages with a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedTaskMetadata {
    /// The task identifier this message is associated with.
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// Data associated with a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// The task identifier.
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// Current task state.
    pub status: TaskStatus,
    /// Optional human-readable message describing the current task state.
    #[serde(rename = "statusMessage", skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    /// ISO 8601 timestamp when the task was created.
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// ISO 8601 timestamp when the task was last updated.
    #[serde(rename = "lastUpdatedAt")]
    pub last_updated_at: String,
    /// Actual retention duration from creation in milliseconds, null for unlimited.
    pub ttl: Option<u64>,
    /// Suggested polling interval in milliseconds.
    #[serde(rename = "pollInterval", skip_serializing_if = "Option::is_none")]
    pub poll_interval: Option<u64>,
}

/// A response to a task-augmented request.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskResult {
    pub task: Task,
}

/// Parameters for tasks/get, tasks/result, and tasks/cancel requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskIdParams {
    /// The task identifier to query.
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// Parameters for a tasks/get request.
pub type GetTaskRequestParams = TaskIdParams;

/// Parameters for a tasks/result request.
pub type GetTaskPayloadRequestParams = TaskIdParams;

/// Parameters for a tasks/cancel request.
pub type CancelTaskRequestParams = TaskIdParams;

/// The response to a tasks/get request.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskResult {
    #[serde(flatten)]
    pub task: Task,
}

/// The response to a tasks/result request.
pub type GetTaskPayloadResult = Result;

/// The response to a tasks/cancel request.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelTaskResult {
    #[serde(flatten)]
    pub task: Task,
}

/// The response to a tasks/list request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTasksResult {
    #[serde(flatten)]
    pub page: PaginatedResult,
    pub tasks: Vec<Task>,
}

impl ListTasksResult {
    /// Create an empty task list.
    pub fn new() -> Self {
        Self {
            page: PaginatedResult {
                next_cursor: None,
                _meta: None,
            },
            tasks: Vec::new(),
        }
    }

    /// Add a task to the result list.
    pub fn with_task(mut self, task: Task) -> Self {
        self.tasks.push(task);
        self
    }

    /// Add multiple tasks to the result list.
    pub fn with_tasks(mut self, tasks: impl IntoIterator<Item = Task>) -> Self {
        self.tasks.extend(tasks);
        self
    }

    /// Set the pagination cursor for the next page.
    pub fn with_cursor(mut self, cursor: impl Into<super::Cursor>) -> Self {
        self.page.next_cursor = Some(cursor.into());
        self
    }
}

impl Default for ListTasksResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Parameters for a `notifications/tasks/status` notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusNotificationParams {
    #[serde(flatten)]
    pub notification: NotificationParams,
    #[serde(flatten)]
    pub task: Task,
}
