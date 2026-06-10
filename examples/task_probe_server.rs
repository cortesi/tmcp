//! Stdio MCP server for probing task-augmented tool support.
//!
//! This example is intentionally small and deterministic so MCP hosts can be
//! checked for the complete task flow: task-augmented `tools/call`,
//! `tasks/get`, `tasks/result`, `tasks/list`, and `tasks/cancel`.

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde_json::{Value, json};
use tmcp::{
    Error, Result, Server, ServerCtx, mcp_server,
    schema::{
        self, CallToolResult, CancelTaskResult, CreateTaskResult, GetTaskResult, ListTasksResult,
        Task, TaskMetadata, TaskStatus,
    },
    tool, tool_params,
};

/// Input accepted by the task probe tool.
#[derive(Debug)]
#[tool_params]
struct ProbeParams {
    /// Token echoed in the final task result.
    token: String,
}

#[derive(Debug, Clone)]
/// Stored probe task and its final response token.
struct TaskEntry {
    /// Task record returned by task lifecycle methods.
    task: Task,
    /// Token echoed by `tasks/result`.
    token: String,
}

#[derive(Debug, Default)]
/// Mutable state for issued probe tasks.
struct ProbeState {
    /// Monotonic task id counter.
    next_task_id: u64,
    /// Task entries keyed by task id.
    tasks: BTreeMap<String, TaskEntry>,
}

#[derive(Debug)]
/// Task probe server state.
struct TaskProbeServer {
    /// Shared task store.
    state: Mutex<ProbeState>,
    /// Optional JSONL request log path.
    log: Option<PathBuf>,
}

#[mcp_server(
    name = "task_probe_server",
    get_task_fn = get_task,
    get_task_payload_fn = get_task_payload,
    list_tasks_fn = list_tasks,
    cancel_task_fn = cancel_task
)]
/// MCP server exposing a required task probe tool.
impl TaskProbeServer {
    /// Create a task probe server.
    fn new(log: Option<PathBuf>) -> Self {
        Self {
            state: Mutex::new(ProbeState::default()),
            log,
        }
    }

    /// Lock the task store.
    fn lock_state(&self) -> Result<MutexGuard<'_, ProbeState>> {
        self.state
            .lock()
            .map_err(|err| Error::InternalError(format!("task state lock poisoned: {err}")))
    }

    /// Append a JSONL event when logging is enabled.
    fn record(&self, event: &str, payload: &Value) -> Result<()> {
        let Some(path) = &self.log else {
            return Ok(());
        };
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        serde_json::to_writer(
            &mut file,
            &json!({
                "event": event,
                "payload": payload,
            }),
        )?;
        writeln!(file)?;
        Ok(())
    }

    /// Convert a task entry into a task result payload.
    fn task_result_payload(entry: &TaskEntry) -> Result<schema::GetTaskPayloadResult> {
        let result =
            CallToolResult::new().with_text_content(format!("task-probe-ok:{}", entry.token));
        let serde_json::Value::Object(object) = serde_json::to_value(result)? else {
            return Err(Error::Protocol(
                "task result payload was not an object".to_string(),
            ));
        };
        Ok(schema::JSONRPCResult {
            _meta: Some(serde_json::Map::from_iter([(
                "io.modelcontextprotocol/related-task".to_string(),
                json!({ "taskId": entry.task.task_id }),
            )])),
            other: object,
        })
    }

    #[tool(task_support = "required")]
    /// Creates a probe task.
    async fn required_task_probe(
        &self,
        _context: &ServerCtx,
        task: TaskMetadata,
        params: ProbeParams,
    ) -> Result<CreateTaskResult> {
        self.record(
            "tools/call",
            &json!({
                "tool": "required_task_probe",
                "task": task,
                "token": params.token,
            }),
        )?;

        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut state = self.lock_state()?;
        state.next_task_id += 1;
        let task_id = format!("task-probe-{}", state.next_task_id);
        let task = Task {
            task_id: task_id.clone(),
            status: TaskStatus::Working,
            status_message: Some("Probe task accepted".to_string()),
            created_at: now.clone(),
            last_updated_at: now,
            ttl: task.ttl,
            poll_interval: None,
        };
        let completed_task = Task {
            status: TaskStatus::Completed,
            status_message: Some("Probe task completed".to_string()),
            ..task.clone()
        };
        state.tasks.insert(
            task_id,
            TaskEntry {
                task: completed_task,
                token: params.token,
            },
        );
        Ok(CreateTaskResult {
            task,
            _meta: None,
            _extra: Default::default(),
        })
    }

    /// Return current task status.
    async fn get_task(&self, _context: &ServerCtx, task_id: String) -> Result<GetTaskResult> {
        self.record("tasks/get", &json!({ "taskId": task_id }))?;
        let state = self.lock_state()?;
        let entry = state
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::InvalidParams(format!("unknown task id: {task_id}")))?;
        Ok(GetTaskResult {
            task: entry.task.clone(),
            _meta: None,
            _extra: Default::default(),
        })
    }

    /// Return the final task payload.
    async fn get_task_payload(
        &self,
        _context: &ServerCtx,
        task_id: String,
    ) -> Result<schema::GetTaskPayloadResult> {
        self.record("tasks/result", &json!({ "taskId": task_id }))?;
        let state = self.lock_state()?;
        let entry = state
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::InvalidParams(format!("unknown task id: {task_id}")))?;
        Self::task_result_payload(entry)
    }

    /// List all known probe tasks.
    async fn list_tasks(
        &self,
        _context: &ServerCtx,
        _cursor: Option<schema::Cursor>,
    ) -> Result<ListTasksResult> {
        self.record("tasks/list", &json!({}))?;
        let state = self.lock_state()?;
        Ok(ListTasksResult::new().with_tasks(
            state
                .tasks
                .values()
                .map(|entry| entry.task.clone())
                .collect::<Vec<_>>(),
        ))
    }

    /// Mark a probe task as cancelled.
    async fn cancel_task(&self, _context: &ServerCtx, task_id: String) -> Result<CancelTaskResult> {
        self.record("tasks/cancel", &json!({ "taskId": task_id }))?;
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut state = self.lock_state()?;
        let entry = state
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| Error::InvalidParams(format!("unknown task id: {task_id}")))?;
        entry.task.status = TaskStatus::Cancelled;
        entry.task.status_message = Some("Probe task cancelled".to_string());
        entry.task.last_updated_at = now;
        Ok(CancelTaskResult {
            task: entry.task.clone(),
            _meta: None,
            _extra: Default::default(),
        })
    }
}

#[derive(Parser)]
#[command(name = "task_probe_server")]
#[command(about = "Stdio MCP server for probing task-augmented tool support")]
/// CLI options for the task probe server.
struct Cli {
    /// Optional JSONL file recording task-related requests.
    #[arg(long)]
    log: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    Server::new(move || TaskProbeServer::new(cli.log.clone()))
        .serve_stdio()
        .await
}
