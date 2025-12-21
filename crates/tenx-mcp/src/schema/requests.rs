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
pub(crate) enum ClientRequest {
    #[serde(rename = "ping")]
    /// Ping the server.
    Ping,
    #[serde(rename = "initialize")]
    /// Initialize a new session with the server.
    Initialize {
        /// The latest version of the Model Context Protocol that the client
        /// supports. The client MAY decide to support older versions as well.
        #[serde(rename = "protocolVersion")]
        protocol_version: String,
        /// Client capabilities advertised to the server.
        capabilities: ClientCapabilities,
        #[serde(rename = "clientInfo")]
        /// Client implementation information.
        client_info: Implementation,
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
    },
    #[serde(rename = "logging/setLevel")]
    /// Set the server logging level.
    SetLevel {
        /// The level of logging that the client wants to receive from the
        /// server.
        level: LoggingLevel,
    },
    #[serde(rename = "prompts/get")]
    /// Get a prompt or prompt template by name.
    GetPrompt {
        /// The name of the prompt or prompt template.
        name: String,
        /// Arguments to use for templating the prompt.
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: Option<Arguments>,
    },
    #[serde(rename = "prompts/list")]
    /// List available prompts.
    ListPrompts {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
    },
    #[serde(rename = "resources/list")]
    /// List available resources.
    ListResources {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
    },
    #[serde(rename = "resources/templates/list")]
    /// List available resource templates.
    ListResourceTemplates {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
    },
    #[serde(rename = "resources/read")]
    /// Read a resource by URI.
    ReadResource {
        /// The URI of the resource to read.
        uri: String,
    },
    #[serde(rename = "resources/subscribe")]
    /// Subscribe to resource updates.
    Subscribe {
        /// The URI of the resource to subscribe to.
        uri: String,
    },
    #[serde(rename = "resources/unsubscribe")]
    /// Unsubscribe from resource updates.
    Unsubscribe {
        /// The URI of the resource to unsubscribe from.
        uri: String,
    },
    #[serde(rename = "tools/call")]
    /// Call a tool by name.
    CallTool {
        /// Tool name to invoke.
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Arguments for the tool call.
        arguments: Option<Arguments>,
    },
    #[serde(rename = "tools/list")]
    /// List available tools.
    ListTools {
        /// An opaque token representing the current pagination position.
        /// If provided, the server should return results starting after this cursor.
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<Cursor>,
    },
}

impl ClientRequest {
    /// Get the method name for this request
    pub fn method(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
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
        #[serde(rename = "requestId")]
        request_id: RequestId,
        /// An optional string describing the reason for the cancellation.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
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
    },

    /// This notification is sent from the client to the server after initialization
    /// has finished.
    #[serde(rename = "notifications/initialized")]
    Initialized,

    /// A notification from the client to the server, informing it that the list of
    /// roots has changed. This notification should be sent whenever the client
    /// adds, removes, or modifies any root. The server should then request an
    /// updated list of roots using the ListRootsRequest.
    #[serde(rename = "notifications/roots/list_changed")]
    RootsListChanged,
}

/// Requests sent from the server to the client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ServerRequest {
    #[serde(rename = "ping")]
    Ping,

    /// A request from the server to sample an LLM via the client. The client has
    /// full discretion over which model to select. The client should also inform
    /// the user before beginning sampling, to allow them to inspect the request
    /// (human in the loop) and decide whether to approve it.
    #[serde(rename = "sampling/createMessage")]
    CreateMessage(Box<CreateMessageParams>),

    #[serde(rename = "roots/list")]
    ListRoots,

    /// A request from the server to elicit additional information from the client.
    /// This allows servers to ask for user input during execution.
    #[serde(rename = "elicitation/create")]
    Elicit(ElicitParams),
}

impl ServerRequest {
    /// Get the method name for this request
    pub fn method(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::CreateMessage { .. } => "sampling/createMessage",
            Self::ListRoots => "roots/list",
            Self::Elicit { .. } => "elicitation/create",
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
        #[serde(rename = "requestId")]
        request_id: RequestId,
        /// An optional string describing the reason for the cancellation.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
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
    },

    /// A notification from the server to the client, informing it that a resource
    /// has changed and may need to be read again. This should only be sent if the
    /// client previously sent a resources/subscribe request.
    #[serde(rename = "notifications/resources/updated")]
    ResourceUpdated {
        /// The URI of the resource that has been updated.
        uri: String,
    },

    /// An optional notification from the server to the client, informing it that
    /// the list of resources it can read from has changed. This may be issued by
    /// servers without any previous subscription from the client.
    #[serde(rename = "notifications/resources/list_changed")]
    ResourceListChanged,

    /// An optional notification from the server to the client, informing it that
    /// the list of tools it offers has changed. This may be issued by servers
    /// without any previous subscription from the client.
    #[serde(rename = "notifications/tools/list_changed")]
    ToolListChanged,

    /// An optional notification from the server to the client, informing it that
    /// the list of prompts it offers has changed. This may be issued by servers
    /// without any previous subscription from the client.
    #[serde(rename = "notifications/prompts/list_changed")]
    PromptListChanged,
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

        assert_eq!(schema.schema_type, "object");

        let properties = schema.properties.expect("Should have properties");
        assert!(properties.contains_key("name"));
        assert!(properties.contains_key("age"));
        assert!(properties.contains_key("email"));

        let required = schema.required.expect("Should have required fields");
        assert!(required.contains(&"name".to_string()));
        assert!(required.contains(&"age".to_string()));
        assert!(!required.contains(&"email".to_string()));
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

        assert_eq!(schema.schema_type, "object");

        let properties = schema.properties.expect("Should have properties");
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
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "tools/list");
        assert_eq!(json["cursor"], "test-cursor");

        // Test ListTools without cursor
        let request = ClientRequest::ListTools { cursor: None };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "tools/list");
        assert!(!json.as_object().unwrap().contains_key("cursor"));

        // Test ListResources with cursor
        let request = ClientRequest::ListResources {
            cursor: Some("res-cursor".into()),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "resources/list");
        assert_eq!(json["cursor"], "res-cursor");

        // Test ListPrompts with cursor
        let request = ClientRequest::ListPrompts {
            cursor: Some("prompt-cursor".into()),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "prompts/list");
        assert_eq!(json["cursor"], "prompt-cursor");

        // Test ListResourceTemplates with cursor
        let request = ClientRequest::ListResourceTemplates {
            cursor: Some("template-cursor".into()),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "resources/templates/list");
        assert_eq!(json["cursor"], "template-cursor");
    }

    #[test]
    fn test_client_capabilities_elicitation() {
        let caps = ClientCapabilities::new().with_elicitation();
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
            reference: Reference::Resource(ResourceReference {
                uri: "test://resource".to_string(),
            }),
            argument: ArgumentInfo {
                name: "arg".to_string(),
                value: "value".to_string(),
            },
            context: Some(CompleteContext::new().add_argument("sessionId", "123")),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["context"]["arguments"]["sessionId"], "123");
    }
}

// ============================================================================
// NEW PROTOCOL STRUCTS FROM 2025-06-18 SCHEMA
// ============================================================================

/// A resource that the server is capable of reading, included in a prompt or tool call result.
///
/// Note: resource links returned by tools are not guaranteed to appear in the results of `resources/list` requests.
// Extends Resource (which extends BaseMetadata)
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLink {
    #[serde(rename = "type")]
    pub link_type: String, // Should be "resource_link"

    /// The URI of this resource.
    pub uri: String,

    /// A description of what this resource represents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The MIME type of this resource, if known.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,

    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// The size of the raw resource content, in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,

    /// Intended for programmatic or logical use, but used as a display name in past specs or fallback (if title isn't present).
    pub name: String,

    /// Intended for UI and end-user contexts — optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    ///
    /// If not provided, the name should be used for display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

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
    #[serde(rename = "requestId")]
    pub request_id: RequestId,

    /// An optional string describing the reason for the cancellation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
    pub arguments: Option<Arguments>,

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
    pub params: ElicitParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitParams {
    /// The message to present to the user.
    pub message: String,

    /// A restricted subset of JSON Schema.
    #[serde(rename = "requestedSchema")]
    pub requested_schema: ElicitSchema,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitSchema {
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
}

/// Restricted schema definitions that only allow primitive types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PrimitiveSchemaDefinition {
    #[serde(rename = "string")]
    String(StringSchema),
    #[serde(rename = "number")]
    Number(NumberSchema),
    #[serde(rename = "integer")]
    Integer(NumberSchema),
    #[serde(rename = "boolean")]
    Boolean(BooleanSchema),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringSchema {
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

    // For enum support
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,

    #[serde(rename = "enumNames", skip_serializing_if = "Option::is_none")]
    pub enum_names: Option<Vec<String>>,
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
pub struct NumberSchema {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BooleanSchema {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}
