use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::*;
use crate::{Arguments, macros::with_open_meta, request_handler::RequestMethod};

// Messages sent from the client to the server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
/// Requests issued by the client.
pub enum ClientRequest {
    #[serde(rename = "ping")]
    /// Ping the server.
    Ping {
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "initialize")]
    /// Initialize a new session with the server.
    Initialize {
        /// The latest version of the Model Context Protocol that the client
        /// supports. The client MAY decide to support older versions as well.
        #[serde(rename = "protocolVersion")]
        protocol_version: ProtocolVersion,
        /// Client capabilities advertised to the server.
        capabilities: Box<ClientCapabilities>,
        #[serde(rename = "clientInfo")]
        /// Client implementation information.
        client_info: Implementation,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "completion/complete")]
    /// Request a completion result.
    Complete {
        #[serde(rename = "ref")]
        /// Reference for the completion request.
        reference: Reference,
        /// The argument's information
        argument: ArgumentInfo,
        /// Additional context for the completion request
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<CompleteContext>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "logging/setLevel")]
    /// Set the server logging level.
    SetLevel {
        /// The level of logging that the client wants to receive from the
        /// server.
        level: LoggingLevel,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "prompts/get")]
    /// Get a prompt or prompt template by name.
    GetPrompt {
        /// The name of the prompt or prompt template.
        name: String,
        /// Arguments to use for templating the prompt.
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: Option<HashMap<String, String>>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "prompts/list")]
    /// List available prompts.
    ListPrompts {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "resources/list")]
    /// List available resources.
    ListResources {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "resources/templates/list")]
    /// List available resource templates.
    ListResourceTemplates {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "resources/read")]
    /// Read a resource by URI.
    ReadResource {
        /// The URI of the resource to read.
        uri: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "resources/subscribe")]
    /// Subscribe to resource updates.
    Subscribe {
        /// The URI of the resource to subscribe to.
        uri: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "resources/unsubscribe")]
    /// Unsubscribe from resource updates.
    Unsubscribe {
        /// The URI of the resource to unsubscribe from.
        uri: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tools/call")]
    /// Call a tool by name.
    CallTool {
        /// Tool name to invoke.
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Arguments for the tool call.
        arguments: Option<Arguments>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Task augmentation metadata for the tool call.
        task: Option<TaskMetadata>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tools/list")]
    /// List available tools.
    ListTools {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tasks/get")]
    /// Retrieve the state of a task.
    GetTask {
        /// The task identifier to query.
        #[serde(rename = "taskId")]
        task_id: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tasks/result")]
    /// Retrieve the result of a completed task.
    GetTaskPayload {
        /// The task identifier to retrieve results for.
        #[serde(rename = "taskId")]
        task_id: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tasks/list")]
    /// List tasks.
    ListTasks {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tasks/cancel")]
    /// Cancel a task.
    CancelTask {
        /// The task identifier to cancel.
        #[serde(rename = "taskId")]
        task_id: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
}

impl ClientRequest {
    /// Create a new Ping request
    pub fn ping() -> Self {
        Self::Ping { _meta: None }
    }

    /// Create a new Initialize request
    pub fn initialize(
        protocol_version: ProtocolVersion,
        capabilities: ClientCapabilities,
        client_info: Implementation,
    ) -> Self {
        Self::Initialize {
            protocol_version,
            capabilities: Box::new(capabilities),
            client_info,
            _meta: None,
        }
    }

    /// Create a new Complete request
    pub fn complete(
        reference: Reference,
        argument: ArgumentInfo,
        context: Option<CompleteContext>,
    ) -> Self {
        Self::Complete {
            reference,
            argument,
            context,
            _meta: None,
        }
    }

    /// Create a new SetLevel request
    pub fn set_level(level: LoggingLevel) -> Self {
        Self::SetLevel { level, _meta: None }
    }

    /// Create a new GetPrompt request
    pub fn get_prompt(name: impl Into<String>, arguments: Option<HashMap<String, String>>) -> Self {
        Self::GetPrompt {
            name: name.into(),
            arguments,
            _meta: None,
        }
    }

    /// Create a new ListPrompts request
    pub fn list_prompts(cursor: Option<Cursor>) -> Self {
        Self::ListPrompts {
            cursor,
            _meta: None,
        }
    }

    /// Create a new ListResources request
    pub fn list_resources(cursor: Option<Cursor>) -> Self {
        Self::ListResources {
            cursor,
            _meta: None,
        }
    }

    /// Create a new ListResourceTemplates request
    pub fn list_resource_templates(cursor: Option<Cursor>) -> Self {
        Self::ListResourceTemplates {
            cursor,
            _meta: None,
        }
    }

    /// Create a new ReadResource request
    pub fn read_resource(uri: impl Into<String>) -> Self {
        Self::ReadResource {
            uri: uri.into(),
            _meta: None,
        }
    }

    /// Create a new Subscribe request
    pub fn subscribe(uri: impl Into<String>) -> Self {
        Self::Subscribe {
            uri: uri.into(),
            _meta: None,
        }
    }

    /// Create a new Unsubscribe request
    pub fn unsubscribe(uri: impl Into<String>) -> Self {
        Self::Unsubscribe {
            uri: uri.into(),
            _meta: None,
        }
    }

    /// Create a new CallTool request
    pub fn call_tool(
        name: impl Into<String>,
        arguments: Option<Arguments>,
        task: Option<TaskMetadata>,
    ) -> Self {
        Self::CallTool {
            name: name.into(),
            arguments,
            task,
            _meta: None,
        }
    }

    /// Create a new ListTools request
    pub fn list_tools(cursor: Option<Cursor>) -> Self {
        Self::ListTools {
            cursor,
            _meta: None,
        }
    }

    /// Create a new GetTask request
    pub fn get_task(task_id: impl Into<String>) -> Self {
        Self::GetTask {
            task_id: task_id.into(),
            _meta: None,
        }
    }

    /// Create a new GetTaskPayload request
    pub fn get_task_payload(task_id: impl Into<String>) -> Self {
        Self::GetTaskPayload {
            task_id: task_id.into(),
            _meta: None,
        }
    }

    /// Create a new ListTasks request
    pub fn list_tasks(cursor: Option<Cursor>) -> Self {
        Self::ListTasks {
            cursor,
            _meta: None,
        }
    }

    /// Create a new CancelTask request
    pub fn cancel_task(task_id: impl Into<String>) -> Self {
        Self::CancelTask {
            task_id: task_id.into(),
            _meta: None,
        }
    }

    /// Get the method name for this request
    pub fn method(&self) -> &'static str {
        match self {
            Self::Ping { .. } => "ping",
            Self::Initialize { .. } => "initialize",
            Self::Complete { .. } => "completion/complete",
            Self::SetLevel { .. } => "logging/setLevel",
            Self::GetPrompt { .. } => "prompts/get",
            Self::ListPrompts { .. } => "prompts/list",
            Self::ListResources { .. } => "resources/list",
            Self::ListResourceTemplates { .. } => "resources/templates/list",
            Self::ReadResource { .. } => "resources/read",
            Self::Subscribe { .. } => "resources/subscribe",
            Self::Unsubscribe { .. } => "resources/unsubscribe",
            Self::CallTool { .. } => "tools/call",
            Self::ListTools { .. } => "tools/list",
            Self::GetTask { .. } => "tasks/get",
            Self::GetTaskPayload { .. } => "tasks/result",
            Self::ListTasks { .. } => "tasks/list",
            Self::CancelTask { .. } => "tasks/cancel",
        }
    }
}

impl RequestMethod for ClientRequest {
    fn method(&self) -> &'static str {
        self.method()
    }
}

/// Notifications sent from the client to the server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ClientNotification {
    // Cancellation
    /// This notification can be sent by either side to indicate that it is
    /// cancelling a previously-issued request.
    ///
    /// The request SHOULD still be in-flight, but due to communication latency, it
    /// is always possible that this notification MAY arrive after the request has
    /// already finished.
    ///
    /// This notification indicates that the result will be unused, so any
    /// associated processing SHOULD cease.
    ///
    /// A client MUST NOT attempt to cancel its `initialize` request.
    #[serde(rename = "notifications/cancelled")]
    Cancelled {
        /// The ID of the request to cancel.
        #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        /// An optional string describing the reason for the cancellation.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },
    /// An out-of-band progress update for a long-running request.
    #[serde(rename = "notifications/progress")]
    Progress {
        /// The progress token which was given in the initial request.
        #[serde(rename = "progressToken")]
        progress_token: ProgressToken,
        /// The progress thus far.
        progress: f64,
        /// Total number of items to process, if known.
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<f64>,
        /// An optional message describing the current progress.
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },

    /// This notification is sent from the client to the server after initialization
    /// has finished.
    #[serde(rename = "notifications/initialized")]
    Initialized {
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },

    /// A notification from the client to the server, informing it that the list of
    /// roots has changed. This notification should be sent whenever the client
    /// adds, removes, or modifies any root. The server should then request an
    /// updated list of roots with a `roots/list` request.
    #[serde(rename = "notifications/roots/list_changed")]
    RootsListChanged {
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },

    /// An optional notification informing that a task's status has changed.
    #[serde(rename = "notifications/tasks/status")]
    TaskStatus {
        /// The task whose status changed.
        #[serde(flatten)]
        params: TaskStatusNotificationParams,
    },
}

impl ClientNotification {
    /// Create a new Cancelled notification
    pub fn cancelled(request_id: Option<RequestId>, reason: Option<String>) -> Self {
        Self::Cancelled {
            request_id,
            reason,
            _meta: None,
        }
    }

    /// Create a new Progress notification
    pub fn progress(
        progress_token: ProgressToken,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    ) -> Self {
        Self::Progress {
            progress_token,
            progress,
            total,
            message,
            _meta: None,
        }
    }

    /// Create a new Initialized notification
    pub fn initialized() -> Self {
        Self::Initialized { _meta: None }
    }

    /// Create a new RootsListChanged notification
    pub fn roots_list_changed() -> Self {
        Self::RootsListChanged { _meta: None }
    }

    /// Create a new TaskStatus notification
    pub fn task_status(params: TaskStatusNotificationParams) -> Self {
        Self::TaskStatus { params }
    }
}

/// Requests sent from the server to the client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ServerRequest {
    /// Ping the client.
    #[serde(rename = "ping")]
    Ping {
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },

    /// A request from the server to sample an LLM via the client. The client has
    /// full discretion over which model to select. The client should also inform
    /// the user before beginning sampling, to allow them to inspect the request
    /// (human in the loop) and decide whether to approve it.
    #[serde(rename = "sampling/createMessage")]
    CreateMessage(Box<CreateMessageParams>),

    /// Request the client's filesystem roots.
    #[serde(rename = "roots/list")]
    ListRoots {
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },

    /// A request from the server to elicit additional information from the client.
    /// This allows servers to ask for user input during execution.
    #[serde(rename = "elicitation/create")]
    Elicit(Box<ElicitRequestParams>),

    #[serde(rename = "tasks/get")]
    /// Retrieve the state of a task.
    GetTask {
        /// The task identifier to query.
        #[serde(rename = "taskId")]
        task_id: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },

    #[serde(rename = "tasks/result")]
    /// Retrieve the result of a completed task.
    GetTaskPayload {
        /// The task identifier to retrieve results for.
        #[serde(rename = "taskId")]
        task_id: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },

    #[serde(rename = "tasks/list")]
    /// List tasks.
    ListTasks {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },

    #[serde(rename = "tasks/cancel")]
    /// Cancel a task.
    CancelTask {
        /// The task identifier to cancel.
        #[serde(rename = "taskId")]
        task_id: String,
        /// Request metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
}

impl ServerRequest {
    /// Create a new Ping request
    pub fn ping() -> Self {
        Self::Ping { _meta: None }
    }

    /// Create a new CreateMessage request
    pub fn create_message(params: CreateMessageParams) -> Self {
        Self::CreateMessage(Box::new(params))
    }

    /// Create a new ListRoots request
    pub fn list_roots() -> Self {
        Self::ListRoots { _meta: None }
    }

    /// Create a new Elicit request
    pub fn elicit(params: ElicitRequestParams) -> Self {
        Self::Elicit(Box::new(params))
    }

    /// Create a new GetTask request
    pub fn get_task(task_id: impl Into<String>) -> Self {
        Self::GetTask {
            task_id: task_id.into(),
            _meta: None,
        }
    }

    /// Create a new GetTaskPayload request
    pub fn get_task_payload(task_id: impl Into<String>) -> Self {
        Self::GetTaskPayload {
            task_id: task_id.into(),
            _meta: None,
        }
    }

    /// Create a new ListTasks request
    pub fn list_tasks(cursor: Option<Cursor>) -> Self {
        Self::ListTasks {
            cursor,
            _meta: None,
        }
    }

    /// Create a new CancelTask request
    pub fn cancel_task(task_id: impl Into<String>) -> Self {
        Self::CancelTask {
            task_id: task_id.into(),
            _meta: None,
        }
    }

    /// Get the method name for this request
    pub fn method(&self) -> &'static str {
        match self {
            Self::Ping { .. } => "ping",
            Self::CreateMessage(_) => "sampling/createMessage",
            Self::ListRoots { .. } => "roots/list",
            Self::Elicit(_) => "elicitation/create",
            Self::GetTask { .. } => "tasks/get",
            Self::GetTaskPayload { .. } => "tasks/result",
            Self::ListTasks { .. } => "tasks/list",
            Self::CancelTask { .. } => "tasks/cancel",
        }
    }
}

impl RequestMethod for ServerRequest {
    fn method(&self) -> &'static str {
        self.method()
    }
}

/// Notifications sent from the server to the client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ServerNotification {
    /// This notification can be sent by either side to indicate that it is
    /// cancelling a previously-issued request.
    ///
    /// The request SHOULD still be in-flight, but due to communication latency, it
    /// is always possible that this notification MAY arrive after the request has
    /// already finished.
    ///
    /// This notification indicates that the result will be unused, so any
    /// associated processing SHOULD cease.
    ///
    /// A client MUST NOT attempt to cancel its `initialize` request.
    #[serde(rename = "notifications/cancelled")]
    Cancelled {
        /// The ID of the request to cancel.
        #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        /// An optional string describing the reason for the cancellation.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },
    /// An out-of-band progress update for a long-running request.
    #[serde(rename = "notifications/progress")]
    Progress {
        /// The progress token which was given in the initial request.
        #[serde(rename = "progressToken")]
        progress_token: ProgressToken,
        /// The progress thus far.
        progress: f64,
        /// Total number of items to process, if known.
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<f64>,
        /// An optional message describing the current progress.
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },
    /// Notification of a log message passed from server to client. If no
    /// logging/setLevel request has been sent from the client, the server MAY
    /// decide which messages to send automatically.
    #[serde(rename = "notifications/message")]
    LoggingMessage {
        /// The severity of this log message.
        level: LoggingLevel,
        /// An optional name of the logger issuing this message.
        #[serde(skip_serializing_if = "Option::is_none")]
        logger: Option<String>,
        /// The data to be logged.
        data: Value,
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },

    /// A notification from the server to the client, informing it that a resource
    /// has changed and may need to be read again. This should only be sent if the
    /// client previously sent a resources/subscribe request.
    #[serde(rename = "notifications/resources/updated")]
    ResourceUpdated {
        /// The URI of the resource that has been updated.
        uri: String,
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },

    /// An optional notification from the server to the client, informing it that
    /// the list of resources it can read from has changed. This may be issued by
    /// servers without any previous subscription from the client.
    #[serde(rename = "notifications/resources/list_changed")]
    ResourceListChanged {
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },

    /// An optional notification from the server to the client, informing it that
    /// the list of tools it offers has changed. This may be issued by servers
    /// without any previous subscription from the client.
    #[serde(rename = "notifications/tools/list_changed")]
    ToolListChanged {
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },

    /// An optional notification from the server to the client, informing it that
    /// the list of prompts it offers has changed. This may be issued by servers
    /// without any previous subscription from the client.
    #[serde(rename = "notifications/prompts/list_changed")]
    PromptListChanged {
        /// Notification metadata reserved by the protocol.
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<Map<String, Value>>,
    },

    /// An optional notification from the server to the client, informing it of
    /// completion of an out-of-band elicitation request.
    #[serde(rename = "notifications/elicitation/complete")]
    ElicitationComplete {
        /// The ID of the elicitation that completed.
        #[serde(rename = "elicitationId")]
        elicitation_id: String,
    },

    /// An optional notification informing that a task's status has changed.
    #[serde(rename = "notifications/tasks/status")]
    TaskStatus {
        /// The task whose status changed.
        #[serde(flatten)]
        params: TaskStatusNotificationParams,
    },
}

impl ServerNotification {
    /// Create a new Cancelled notification
    pub fn cancelled(request_id: Option<RequestId>, reason: Option<String>) -> Self {
        Self::Cancelled {
            request_id,
            reason,
            _meta: None,
        }
    }

    /// Create a new Progress notification
    pub fn progress(
        progress_token: ProgressToken,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    ) -> Self {
        Self::Progress {
            progress_token,
            progress,
            total,
            message,
            _meta: None,
        }
    }

    /// Create a new LoggingMessage notification
    pub fn logging_message(level: LoggingLevel, logger: Option<String>, data: Value) -> Self {
        Self::LoggingMessage {
            level,
            logger,
            data,
            _meta: None,
        }
    }

    /// Create a new ResourceUpdated notification
    pub fn resource_updated(uri: impl Into<String>) -> Self {
        Self::ResourceUpdated {
            uri: uri.into(),
            _meta: None,
        }
    }

    /// Create a new ResourceListChanged notification
    pub fn resource_list_changed() -> Self {
        Self::ResourceListChanged { _meta: None }
    }

    /// Create a new ToolListChanged notification
    pub fn tool_list_changed() -> Self {
        Self::ToolListChanged { _meta: None }
    }

    /// Create a new PromptListChanged notification
    pub fn prompt_list_changed() -> Self {
        Self::PromptListChanged { _meta: None }
    }

    /// Create a new ElicitationComplete notification
    pub fn elicitation_complete(elicitation_id: impl Into<String>) -> Self {
        Self::ElicitationComplete {
            elicitation_id: elicitation_id.into(),
        }
    }

    /// Create a new TaskStatus notification
    pub fn task_status(params: TaskStatusNotificationParams) -> Self {
        Self::TaskStatus { params }
    }
}

#[cfg(test)]
mod tests {
    use schemars::JsonSchema;

    use super::*;

    #[derive(JsonSchema, Serialize)]
    struct TestInput {
        name: String,
        age: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<String>,
    }

    #[test]
    fn test_tool_input_schema_from_json_schema() {
        let schema = ToolSchema::from_json_schema::<TestInput>();

        assert_eq!(schema.schema_type(), Some("object"));

        let properties = schema.properties().expect("Should have properties");
        assert!(properties.contains_key("name"));
        assert!(properties.contains_key("age"));
        assert!(properties.contains_key("email"));

        let required = schema.required().expect("Should have required fields");
        assert!(required.contains(&"name"));
        assert!(required.contains(&"age"));
        assert!(!required.contains(&"email"));
    }

    #[derive(JsonSchema, Serialize)]
    struct ComplexInput {
        id: i64,
        tags: Vec<String>,
        metadata: HashMap<String, String>,
    }

    #[test]
    fn test_complex_schema_conversion() {
        let schema = ToolSchema::from_json_schema::<ComplexInput>();

        assert_eq!(schema.schema_type(), Some("object"));

        let properties = schema.properties().expect("Should have properties");
        assert!(properties.contains_key("id"));
        assert!(properties.contains_key("tags"));
        assert!(properties.contains_key("metadata"));

        // Verify array type for tags
        let tags_schema = &properties["tags"];
        assert_eq!(
            tags_schema.get("type").and_then(|v| v.as_str()),
            Some("array")
        );

        // Verify object type for metadata
        let metadata_schema = &properties["metadata"];
        assert_eq!(
            metadata_schema.get("type").and_then(|v| v.as_str()),
            Some("object")
        );
    }

    #[test]
    fn test_paginated_request_serialization() {
        // Test ListTools with cursor
        let request = ClientRequest::ListTools {
            cursor: Some("test-cursor".into()),
            _meta: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "tools/list");
        assert_eq!(json["cursor"], "test-cursor");

        // Test ListTools without cursor
        let request = ClientRequest::ListTools {
            cursor: None,
            _meta: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "tools/list");
        assert!(!json.as_object().unwrap().contains_key("cursor"));

        // Test ListResources with cursor
        let request = ClientRequest::ListResources {
            cursor: Some("res-cursor".into()),
            _meta: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "resources/list");
        assert_eq!(json["cursor"], "res-cursor");

        // Test ListPrompts with cursor
        let request = ClientRequest::ListPrompts {
            cursor: Some("prompt-cursor".into()),
            _meta: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "prompts/list");
        assert_eq!(json["cursor"], "prompt-cursor");

        // Test ListResourceTemplates with cursor
        let request = ClientRequest::ListResourceTemplates {
            cursor: Some("template-cursor".into()),
            _meta: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "resources/templates/list");
        assert_eq!(json["cursor"], "template-cursor");
    }

    #[test]
    fn test_client_capabilities_elicitation() {
        let caps = ClientCapabilities::default().with_elicitation();
        let json = serde_json::to_value(&caps).unwrap();
        assert!(json["elicitation"].is_object());
    }

    #[test]
    fn test_tool_output_schema() {
        let tool =
            Tool::new("test_tool", ToolSchema::default()).with_output_schema(ToolSchema::default());
        let json = serde_json::to_value(&tool).unwrap();
        assert!(json["outputSchema"].is_object());
    }

    #[test]
    fn test_call_tool_result_structured_content() {
        let structured = serde_json::json!({"type": "table"});
        let result = CallToolResult::new().with_structured_content(structured.clone());
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["structuredContent"], structured);
    }

    #[test]
    fn test_annotations_last_modified() {
        let annotations = Annotations {
            audience: None,
            priority: None,
            last_modified: Some("2024-01-15T10:30:00Z".to_string()),
        };
        let json = serde_json::to_value(&annotations).unwrap();
        assert_eq!(json["lastModified"], "2024-01-15T10:30:00Z");
    }

    #[test]
    fn test_complete_request_context() {
        let request = ClientRequest::Complete {
            reference: Reference::Resource(ResourceTemplateReference {
                uri: "test://resource".to_string(),
            }),
            argument: ArgumentInfo {
                name: "arg".to_string(),
                value: "value".to_string(),
            },
            context: Some(CompleteContext::new().add_argument("sessionId", "123")),
            _meta: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["context"]["arguments"]["sessionId"], "123");
    }
}

/// Base interface for paginated results
#[with_open_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResult {
    /// An opaque token representing the pagination position after the last returned result.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

// Completion-related types

/// Additional, optional context for a completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteContext {
    /// Previously-resolved variables in a URI template or prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, String>>,
}

impl CompleteContext {
    /// Create a new CompleteContext with no arguments
    pub fn new() -> Self {
        Self { arguments: None }
    }

    /// Set the arguments, replacing any existing ones
    pub fn with_arguments(mut self, arguments: HashMap<String, String>) -> Self {
        self.arguments = Some(arguments);
        self
    }

    /// Add a single argument to the context
    pub fn add_argument(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let arguments = self.arguments.get_or_insert_with(HashMap::new);
        arguments.insert(key.into(), value.into());
        self
    }
}

impl Default for CompleteContext {
    fn default() -> Self {
        Self::new()
    }
}

// Elicitation-related types

/// Parameters for an elicitation/create request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ElicitRequestParams {
    /// Form-based elicitation collecting structured user input.
    Form(ElicitRequestFormParams),
    /// URL-based elicitation directing the user to a web page.
    Url(ElicitRequestURLParams),
}

/// Parameters for a form-based elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitRequestFormParams {
    /// The elicitation mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<ElicitMode>,

    /// The message to present to the user describing what information is being requested.
    pub message: String,

    /// A restricted subset of JSON Schema.
    #[serde(rename = "requestedSchema")]
    pub requested_schema: ElicitSchema,

    /// Task augmentation metadata for the elicitation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,

    /// Request metadata reserved by the protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// Parameters for a URL-based elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitRequestURLParams {
    /// The elicitation mode.
    pub mode: ElicitMode,

    /// The message to present to the user explaining why the interaction is needed.
    pub message: String,

    /// The ID of the elicitation, which must be unique within the context of the server.
    #[serde(rename = "elicitationId")]
    pub elicitation_id: String,

    /// The URL that the user should navigate to.
    pub url: String,

    /// Task augmentation metadata for the elicitation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,

    /// Request metadata reserved by the protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// The interaction mode of an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitMode {
    /// Collect structured input via a form.
    Form,
    /// Direct the user to a URL.
    Url,
}

/// The restricted JSON Schema describing requested form input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitSchema {
    /// Optional JSON Schema dialect identifier.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// The schema type, always "object".
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Property schemas for each requested field.
    pub properties: HashMap<String, PrimitiveSchemaDefinition>,
    /// Names of required properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
}

/// The client's response to an elicitation request.
#[with_open_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitResult {
    /// The user action in response to the elicitation.
    pub action: ElicitAction,

    /// The submitted form data, only present when action is "accept".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<HashMap<String, ElicitValue>>,
}

/// The user's action in response to an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitAction {
    /// The user submitted the form.
    Accept,
    /// The user explicitly declined.
    Decline,
    /// The user dismissed the request without choosing.
    Cancel,
}

/// A single value submitted in an elicitation form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ElicitValue {
    /// A string value.
    String(String),
    /// A numeric value.
    Number(f64),
    /// A boolean value.
    Boolean(bool),
    /// A list of strings, from multi-select fields.
    StringArray(Vec<String>),
}

/// Restricted schema definitions that only allow primitive types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PrimitiveSchemaDefinition {
    /// An enumerated choice schema.
    Enum(EnumSchema),
    /// A string schema.
    String(StringSchema),
    /// A number or integer schema.
    Number(NumberSchema),
    /// A boolean schema.
    Boolean(BooleanSchema),
}

/// Type tag for string schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StringSchemaType {
    /// The "string" type.
    String,
}

/// Type tag for numeric schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NumberSchemaType {
    /// The "number" type.
    Number,
    /// The "integer" type.
    Integer,
}

/// Type tag for boolean schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BooleanSchemaType {
    /// The "boolean" type.
    Boolean,
}

/// Type tag for array schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArraySchemaType {
    /// The "array" type.
    Array,
}

/// Schema for a string form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringSchema {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    /// Display title for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Minimum string length.
    #[serde(rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u32>,
    /// Maximum string length.
    #[serde(rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    /// Expected string format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<StringFormat>,
    /// Default value for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Expected format of a string form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StringFormat {
    /// An email address.
    Email,
    /// A URI.
    Uri,
    /// A calendar date.
    Date,
    /// A date and time.
    DateTime,
}

/// Schema for a numeric form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberSchema {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: NumberSchemaType,
    /// Display title for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Minimum allowed value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    /// Maximum allowed value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    /// Default value for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<f64>,
}

/// Schema for a boolean form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BooleanSchema {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: BooleanSchemaType,
    /// Display title for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Default value for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

/// One selectable option in a titled enum schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumOption {
    /// The value submitted when this option is selected.
    #[serde(rename = "const")]
    pub value: String,
    /// Display title for the option.
    pub title: String,
}

/// Single-select enum schema whose options have no display titles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UntitledSingleSelectEnumSchema {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    /// Display title for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Allowed values.
    #[serde(rename = "enum")]
    pub values: Vec<String>,
    /// Default value for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Single-select enum schema whose options carry display titles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitledSingleSelectEnumSchema {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    /// Display title for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Selectable options with display titles.
    #[serde(rename = "oneOf")]
    pub options: Vec<EnumOption>,
    /// Default value for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Schema for a single-select enum form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SingleSelectEnumSchema {
    /// Options without display titles.
    Untitled(UntitledSingleSelectEnumSchema),
    /// Options with display titles.
    Titled(TitledSingleSelectEnumSchema),
}

/// Item schema for a multi-select enum without display titles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntitledMultiSelectItems {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    /// Allowed values.
    #[serde(rename = "enum")]
    pub values: Vec<String>,
}

/// Item schema for a multi-select enum with display titles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitledMultiSelectItems {
    /// Selectable options with display titles.
    #[serde(rename = "anyOf")]
    pub options: Vec<EnumOption>,
}

/// Multi-select enum schema whose options have no display titles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntitledMultiSelectEnumSchema {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: ArraySchemaType,
    /// Display title for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Minimum number of selections.
    #[serde(rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u32>,
    /// Maximum number of selections.
    #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    /// Schema for the selectable items.
    pub items: UntitledMultiSelectItems,
    /// Default selections for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<String>>,
}

/// Multi-select enum schema whose options carry display titles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitledMultiSelectEnumSchema {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: ArraySchemaType,
    /// Display title for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Minimum number of selections.
    #[serde(rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u32>,
    /// Maximum number of selections.
    #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    /// Schema for the selectable items.
    pub items: TitledMultiSelectItems,
    /// Default selections for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<String>>,
}

/// Schema for a multi-select enum form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MultiSelectEnumSchema {
    /// Options without display titles.
    Untitled(UntitledMultiSelectEnumSchema),
    /// Options with display titles.
    Titled(TitledMultiSelectEnumSchema),
}

/// Deprecated enum schema using the legacy `enumNames` titling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyTitledEnumSchema {
    /// The schema type tag.
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    /// Display title for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Allowed values.
    #[serde(rename = "enum")]
    pub values: Vec<String>,
    /// Display titles parallel to `values`.
    #[serde(rename = "enumNames", skip_serializing_if = "Option::is_none")]
    pub enum_names: Option<Vec<String>>,
    /// Default value for the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Schema for an enumerated form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnumSchema {
    /// Single-select enum.
    Single(SingleSelectEnumSchema),
    /// Multi-select enum.
    Multi(MultiSelectEnumSchema),
    /// Legacy `enumNames` titling.
    Legacy(LegacyTitledEnumSchema),
}
