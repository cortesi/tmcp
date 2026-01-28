#![allow(missing_docs)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::*;
use crate::{Arguments, macros::with_meta, request_handler::RequestMethod};

// Messages sent from the client to the server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
/// Requests issued by the client.
pub enum ClientRequest {
    #[serde(rename = "ping")]
    /// Ping the server.
    Ping {
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "initialize")]
    /// Initialize a new session with the server.
    Initialize {
        /// The latest version of the Model Context Protocol that the client
        /// supports. The client MAY decide to support older versions as well.
        #[serde(rename = "protocolVersion")]
        protocol_version: String,
        /// Client capabilities advertised to the server.
        capabilities: Box<ClientCapabilities>,
        #[serde(rename = "clientInfo")]
        /// Client implementation information.
        client_info: Implementation,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "logging/setLevel")]
    /// Set the server logging level.
    SetLevel {
        /// The level of logging that the client wants to receive from the
        /// server.
        level: LoggingLevel,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "resources/read")]
    /// Read a resource by URI.
    ReadResource {
        /// The URI of the resource to read.
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "resources/subscribe")]
    /// Subscribe to resource updates.
    Subscribe {
        /// The URI of the resource to subscribe to.
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "resources/unsubscribe")]
    /// Unsubscribe from resource updates.
    Unsubscribe {
        /// The URI of the resource to unsubscribe from.
        uri: String,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tasks/get")]
    /// Retrieve the state of a task.
    GetTask {
        /// The task identifier to query.
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tasks/result")]
    /// Retrieve the result of a completed task.
    GetTaskPayload {
        /// The task identifier to retrieve results for.
        #[serde(rename = "taskId")]
        task_id: String,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },
    #[serde(rename = "tasks/cancel")]
    /// Cancel a task.
    CancelTask {
        /// The task identifier to cancel.
        #[serde(rename = "taskId")]
        task_id: String,
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
        protocol_version: impl Into<String>,
        capabilities: ClientCapabilities,
        client_info: Implementation,
    ) -> Self {
        Self::Initialize {
            protocol_version: protocol_version.into(),
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },

    /// This notification is sent from the client to the server after initialization
    /// has finished.
    #[serde(rename = "notifications/initialized")]
    Initialized {
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },

    /// A notification from the client to the server, informing it that the list of
    /// roots has changed. This notification should be sent whenever the client
    /// adds, removes, or modifies any root. The server should then request an
    /// updated list of roots using the ListRootsRequest.
    #[serde(rename = "notifications/roots/list_changed")]
    RootsListChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },

    /// An optional notification informing that a task's status has changed.
    #[serde(rename = "notifications/tasks/status")]
    TaskStatus {
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
    #[serde(rename = "ping")]
    Ping {
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },

    /// A request from the server to sample an LLM via the client. The client has
    /// full discretion over which model to select. The client should also inform
    /// the user before beginning sampling, to allow them to inspect the request
    /// (human in the loop) and decide whether to approve it.
    #[serde(rename = "sampling/createMessage")]
    CreateMessage(Box<CreateMessageParams>),

    #[serde(rename = "roots/list")]
    ListRoots {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },

    #[serde(rename = "tasks/result")]
    /// Retrieve the result of a completed task.
    GetTaskPayload {
        /// The task identifier to retrieve results for.
        #[serde(rename = "taskId")]
        task_id: String,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<RequestMeta>,
    },

    #[serde(rename = "tasks/cancel")]
    /// Cancel a task.
    CancelTask {
        /// The task identifier to cancel.
        #[serde(rename = "taskId")]
        task_id: String,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },

    /// A notification from the server to the client, informing it that a resource
    /// has changed and may need to be read again. This should only be sent if the
    /// client previously sent a resources/subscribe request.
    #[serde(rename = "notifications/resources/updated")]
    ResourceUpdated {
        /// The URI of the resource that has been updated.
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },

    /// An optional notification from the server to the client, informing it that
    /// the list of resources it can read from has changed. This may be issued by
    /// servers without any previous subscription from the client.
    #[serde(rename = "notifications/resources/list_changed")]
    ResourceListChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },

    /// An optional notification from the server to the client, informing it that
    /// the list of tools it offers has changed. This may be issued by servers
    /// without any previous subscription from the client.
    #[serde(rename = "notifications/tools/list_changed")]
    ToolListChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
    },

    /// An optional notification from the server to the client, informing it that
    /// the list of prompts it offers has changed. This may be issued by servers
    /// without any previous subscription from the client.
    #[serde(rename = "notifications/prompts/list_changed")]
    PromptListChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        _meta: Option<HashMap<String, Value>>,
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

// ============================================================================
// JSON-RPC request/response structs for MCP methods
// ============================================================================

/// This request is sent from the client to the server when it first connects, asking it to begin initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub method: String, // "initialize"
    pub params: InitializeParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    /// The latest version of the Model Context Protocol that the client supports.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: Implementation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// This notification is sent from the client to the server after initialization has finished.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializedNotification {
    pub method: String, // "notifications/initialized"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, Value>>,
}

/// A ping, issued by either the server or the client, to check that the other party is still alive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingRequest {
    pub method: String, // "ping"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, Value>>,
}

/// This notification can be sent by either side to indicate that it is cancelling a previously-issued request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelledNotification {
    pub method: String, // "notifications/cancelled"
    pub params: CancelledParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelledParams {
    /// The ID of the request to cancel.
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,

    /// An optional string describing the reason for the cancellation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<HashMap<String, Value>>,
}

/// An out-of-band notification used to inform the receiver of a progress update for a long-running request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressNotification {
    pub method: String, // "notifications/progress"
    pub params: ProgressParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressParams {
    /// The progress token which was given in the initial request.
    #[serde(rename = "progressToken")]
    pub progress_token: ProgressToken,

    /// The progress thus far. This should increase every time progress is made.
    pub progress: f64,

    /// Total number of items to process (or total progress required), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,

    /// An optional message describing the current progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<HashMap<String, Value>>,
}

/// Base interface for paginated requests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedRequest {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<PaginatedParams>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedParams {
    /// An opaque token representing the current pagination position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// Base interface for paginated results
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResult {
    /// An opaque token representing the pagination position after the last returned result.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

// Resource-related request types

/// Sent from the client to request a list of resources the server has.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourcesRequest {
    pub method: String, // "resources/list"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<PaginatedParams>,
}

/// Sent from the client to request a list of resource templates the server has.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourceTemplatesRequest {
    pub method: String, // "resources/templates/list"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<PaginatedParams>,
}

/// Sent from the client to the server, to read a specific resource URI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceRequest {
    pub method: String, // "resources/read"
    pub params: ReadResourceParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceParams {
    /// The URI of the resource to read.
    pub uri: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// Sent from the client to request resources/updated notifications from the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub method: String, // "resources/subscribe"
    pub params: SubscribeParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeParams {
    /// The URI of the resource to subscribe to.
    pub uri: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// Sent from the client to request cancellation of resources/updated notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsubscribeRequest {
    pub method: String, // "resources/unsubscribe"
    pub params: UnsubscribeParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsubscribeParams {
    /// The URI of the resource to unsubscribe from.
    pub uri: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// An optional notification from the server to the client about resource list changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceListChangedNotification {
    pub method: String, // "notifications/resources/list_changed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, Value>>,
}

/// A notification from the server to the client about a resource update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUpdatedNotification {
    pub method: String, // "notifications/resources/updated"
    pub params: ResourceUpdatedParams,
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUpdatedParams {
    /// The URI of the resource that has been updated.
    pub uri: String,
}

// Prompt-related request types

/// Sent from the client to request a list of prompts and prompt templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPromptsRequest {
    pub method: String, // "prompts/list"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<PaginatedParams>,
}

/// Used by the client to get a prompt provided by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptRequest {
    pub method: String, // "prompts/get"
    pub params: GetPromptParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptParams {
    /// The name of the prompt or prompt template.
    pub name: String,

    /// Arguments to use for templating the prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// An optional notification from the server about prompt list changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptListChangedNotification {
    pub method: String, // "notifications/prompts/list_changed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, Value>>,
}

// Tool-related request types

/// Sent from the client to request a list of tools the server has.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsRequest {
    pub method: String, // "tools/list"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<PaginatedParams>,
}

/// Used by the client to invoke a tool provided by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolRequest {
    pub method: String, // "tools/call"
    pub params: CallToolParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Arguments>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// An optional notification from the server about tool list changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListChangedNotification {
    pub method: String, // "notifications/tools/list_changed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, Value>>,
}

// Logging-related types

/// A request from the client to the server, to enable or adjust logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLevelRequest {
    pub method: String, // "logging/setLevel"
    pub params: SetLevelParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLevelParams {
    /// The level of logging that the client wants to receive from the server.
    pub level: LoggingLevel,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

/// Notification of a log message passed from server to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingMessageNotification {
    pub method: String, // "notifications/message"
    pub params: LoggingMessageParams,
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingMessageParams {
    /// The severity of this log message.
    pub level: LoggingLevel,

    /// An optional name of the logger issuing this message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,

    /// The data to be logged, such as a string message or an object.
    pub data: Value,
}

// Sampling-related types

/// A request from the server to sample an LLM via the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageRequest {
    pub method: String, // "sampling/createMessage"
    pub params: CreateMessageParams,
}

// Completion-related types

/// A request from the client to the server, to ask for completion options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteRequest {
    pub method: String, // "completion/complete"
    pub params: CompleteParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteParams {
    #[serde(rename = "ref")]
    pub reference: Reference,

    /// The argument's information
    pub argument: ArgumentInfo,

    /// Additional, optional context for completions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<CompleteContext>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

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

    /// Create a CompleteContext with the provided arguments
    pub fn with_arguments(arguments: HashMap<String, String>) -> Self {
        Self {
            arguments: Some(arguments),
        }
    }

    /// Add a single argument to the context
    pub fn add_argument(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let arguments = self.arguments.get_or_insert_with(HashMap::new);
        arguments.insert(key.into(), value.into());
        self
    }

    /// Set the arguments, replacing any existing ones
    pub fn set_arguments(mut self, arguments: HashMap<String, String>) -> Self {
        self.arguments = Some(arguments);
        self
    }
}

impl Default for CompleteContext {
    fn default() -> Self {
        Self::new()
    }
}

// Roots-related types

/// Sent from the server to request a list of root URIs from the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRootsRequest {
    pub method: String, // "roots/list"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, Value>>,
}

/// A notification from the client to the server about roots list changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootsListChangedNotification {
    pub method: String, // "notifications/roots/list_changed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, Value>>,
}

// Elicitation-related types

/// A request from the server to elicit additional information from the user via the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitRequest {
    pub method: String, // "elicitation/create"
    pub params: ElicitRequestParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ElicitRequestParams {
    Form(ElicitRequestFormParams),
    Url(ElicitRequestURLParams),
}

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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitMode {
    Form,
    Url,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitSchema {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(rename = "type")]
    pub schema_type: String, // "object"
    pub properties: HashMap<String, PrimitiveSchemaDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
}

/// The client's response to an elicitation request.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitResult {
    /// The user action in response to the elicitation.
    pub action: ElicitAction,

    /// The submitted form data, only present when action is "accept".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<HashMap<String, ElicitValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ElicitValue {
    String(String),
    Number(f64),
    Boolean(bool),
    StringArray(Vec<String>),
}

/// Restricted schema definitions that only allow primitive types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PrimitiveSchemaDefinition {
    Enum(EnumSchema),
    String(StringSchema),
    Number(NumberSchema),
    Boolean(BooleanSchema),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StringSchemaType {
    String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NumberSchemaType {
    Number,
    Integer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BooleanSchemaType {
    Boolean,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArraySchemaType {
    Array,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringSchema {
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u32>,
    #[serde(rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<StringFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StringFormat {
    Email,
    Uri,
    Date,
    DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberSchema {
    #[serde(rename = "type")]
    pub schema_type: NumberSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BooleanSchema {
    #[serde(rename = "type")]
    pub schema_type: BooleanSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumOption {
    #[serde(rename = "const")]
    pub value: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UntitledSingleSelectEnumSchema {
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "enum")]
    pub values: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitledSingleSelectEnumSchema {
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "oneOf")]
    pub options: Vec<EnumOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SingleSelectEnumSchema {
    Untitled(UntitledSingleSelectEnumSchema),
    Titled(TitledSingleSelectEnumSchema),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntitledMultiSelectItems {
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    #[serde(rename = "enum")]
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitledMultiSelectItems {
    #[serde(rename = "anyOf")]
    pub options: Vec<EnumOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntitledMultiSelectEnumSchema {
    #[serde(rename = "type")]
    pub schema_type: ArraySchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u32>,
    #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    pub items: UntitledMultiSelectItems,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitledMultiSelectEnumSchema {
    #[serde(rename = "type")]
    pub schema_type: ArraySchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u32>,
    #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
    pub items: TitledMultiSelectItems,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MultiSelectEnumSchema {
    Untitled(UntitledMultiSelectEnumSchema),
    Titled(TitledMultiSelectEnumSchema),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyTitledEnumSchema {
    #[serde(rename = "type")]
    pub schema_type: StringSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "enum")]
    pub values: Vec<String>,
    #[serde(rename = "enumNames", skip_serializing_if = "Option::is_none")]
    pub enum_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnumSchema {
    Single(SingleSelectEnumSchema),
    Multi(MultiSelectEnumSchema),
    Legacy(LegacyTitledEnumSchema),
}
