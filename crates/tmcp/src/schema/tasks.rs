use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{JSONRPCResult, PaginatedResult};
use crate::macros::{with_meta, with_open_meta};

/// The status of a task.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// The task is executing.
    Working,
    /// The task is waiting for additional input.
    InputRequired,
    /// The task finished successfully.
    Completed,
    /// The task finished with an error.
    Failed,
    /// The task was cancelled before completion.
    Cancelled,
}

/// Metadata for augmenting a request with task execution.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskMetadata {
    /// Requested duration in milliseconds to retain task from creation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u64>,
}

/// Data associated with a task.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[with_open_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskResult {
    /// The task created for the request.
    pub task: Task,
}

/// The response to a tasks/get request.
#[with_open_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskResult {
    /// The current state of the task.
    #[serde(flatten)]
    pub task: Task,
}

/// The response to a tasks/result request.
pub type GetTaskPayloadResult = JSONRPCResult;

/// The response to a tasks/cancel request.
#[with_open_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelTaskResult {
    /// The state of the task after cancellation.
    #[serde(flatten)]
    pub task: Task,
}

/// The response to a tasks/list request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTasksResult {
    /// Pagination state for the listing.
    #[serde(flatten)]
    pub page: PaginatedResult,
    /// Tasks returned in this page.
    pub tasks: Vec<Task>,
}

impl ListTasksResult {
    /// Create an empty task list.
    pub fn new() -> Self {
        Self {
            page: PaginatedResult {
                next_cursor: None,
                _meta: None,
                _extra: Default::default(),
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
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusNotificationParams {
    /// The task whose status changed.
    #[serde(flatten)]
    pub task: Task,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn task_status_notification_params_round_trip_without_duplicate_keys() {
        let wire = json!({
            "taskId": "task-1",
            "status": "working",
            "createdAt": "2025-01-01T00:00:00Z",
            "lastUpdatedAt": "2025-01-01T00:00:01Z",
            "ttl": null,
            "_meta": {"k": "v"},
        });
        let params: TaskStatusNotificationParams = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(params.task.task_id, "task-1");
        assert!(matches!(params.task.status, TaskStatus::Working));

        let out = serde_json::to_string(&params).unwrap();
        for key in ["taskId", "status", "createdAt", "lastUpdatedAt", "ttl"] {
            assert_eq!(
                out.matches(&format!("\"{key}\"")).count(),
                1,
                "key {key} must appear exactly once in {out}"
            );
        }
        let reparsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(reparsed, wire);
    }
}
