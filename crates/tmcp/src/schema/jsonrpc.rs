use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::macros::with_meta;

/// The protocol version immediately preceding the latest supported version.
pub const PREVIOUS_PROTOCOL_VERSION: &str = "2025-06-18";

/// All protocol versions this implementation accepts from peers, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// The most recent protocol version this implementation supports.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
/// JSON-RPC protocol version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// Refers to any valid JSON-RPC object that can be decoded off the wire, or
/// encoded to be sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JSONRPCMessage {
    /// A request expecting a response.
    Request(JSONRPCRequest),
    /// A notification with no response.
    Notification(JSONRPCNotification),
    /// A response to a request.
    Response(JSONRPCResponse),
}

/// A progress token, used to associate progress notifications with the original
/// request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgressToken {
    /// String progress token.
    String(String),
    /// Numeric progress token.
    Number(i64),
}

/// An opaque token used to represent a cursor for pagination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct Cursor(pub String);

impl From<&str> for Cursor {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for Cursor {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&String> for Cursor {
    fn from(s: &String) -> Self {
        Self(s.clone())
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Common params for any request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestParams {
    /// Request metadata reserved by the protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
    /// Method-specific parameters.
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

/// Metadata attached to a request via the reserved `_meta` parameter.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestMeta {
    /// If specified, the caller is requesting out-of-band progress
    /// notifications for this request (as represented by
    /// notifications/progress). The value of this parameter is an opaque token
    /// that will be attached to any subsequent notifications. The receiver is
    /// not obligated to provide these notifications.
    #[serde(rename = "progressToken", skip_serializing_if = "Option::is_none")]
    pub progress_token: Option<ProgressToken>,
    /// Additional metadata fields preserved from the wire.
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

/// The method and params of a JSON-RPC request, without the envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// The method being invoked.
    pub method: String,
    /// Parameters for the method, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<RequestParams>,
}

/// Common params for any notification.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationParams {
    /// This parameter name is reserved by MCP to allow clients and servers to
    /// attach additional metadata to their notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<HashMap<String, Value>>,
    /// Method-specific parameters.
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

/// The method and params of a JSON-RPC notification, without the envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// The notification method.
    pub method: String,
    /// Parameters for the notification, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<NotificationParams>,
}

/// The payload of a successful JSON-RPC response.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCResult {
    /// This result property is reserved by the protocol to allow clients and
    /// servers to attach additional metadata to their responses.
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

/// A uniquely identifying ID for a request in JSON-RPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    /// String request ID.
    String(String),
    /// Numeric request ID.
    Number(i64),
}

impl Display for RequestId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => write!(f, "{s}"),
            Self::Number(n) => write!(f, "{n}"),
        }
    }
}

/// A request that expects a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCRequest {
    /// JSON-RPC protocol version, always "2.0".
    pub jsonrpc: String,
    /// Identifier correlating the request with its response.
    pub id: RequestId,
    /// The method and params of the request.
    #[serde(flatten)]
    pub request: Request,
}

/// A notification which does not expect a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCNotification {
    /// JSON-RPC protocol version, always "2.0".
    pub jsonrpc: String,
    /// The method and params of the notification.
    #[serde(flatten)]
    pub notification: Notification,
}

/// A successful (non-error) response to a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCResultResponse {
    /// JSON-RPC protocol version, always "2.0".
    pub jsonrpc: String,
    /// Identifier of the request this responds to.
    pub id: RequestId,
    /// The result payload.
    pub result: JSONRPCResult,
}

/// A response to a request that indicates an error occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCErrorResponse {
    /// JSON-RPC protocol version, always "2.0".
    pub jsonrpc: String,
    /// Identifier of the request this responds to, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// Details of the error.
    pub error: ErrorObject,
}

/// A response to a request, containing either the result or error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JSONRPCResponse {
    /// A successful response.
    Result(JSONRPCResultResponse),
    /// An error response.
    Error(JSONRPCErrorResponse),
}

// Standard JSON-RPC error codes
/// JSON-RPC parse error code.
pub const PARSE_ERROR: i32 = -32700;
/// JSON-RPC invalid request error code.
pub const INVALID_REQUEST: i32 = -32600;
/// JSON-RPC method not found error code.
pub const METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC invalid params error code.
pub const INVALID_PARAMS: i32 = -32602;
/// JSON-RPC internal error code.
pub const INTERNAL_ERROR: i32 = -32603;

/// MCP-specific JSON-RPC error code indicating a requested resource was not found.
pub const RESOURCE_NOT_FOUND: i32 = -32002;

/// Implementation-specific JSON-RPC error code indicating URL elicitation is required.
pub const URL_ELICITATION_REQUIRED: i32 = -32042;
/// Implementation-specific JSON-RPC error code indicating authorization failed.
pub const AUTHORIZATION_FAILED: i32 = -32041;

/// The error payload of a JSON-RPC error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    /// The error type that occurred.
    pub code: i32,
    /// A short description of the error. The message SHOULD be limited to a
    /// concise single sentence.
    pub message: String,
    /// Additional information about the error. The value of this member is
    /// defined by the sender (e.g. detailed error information, nested
    /// errors etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A response that indicates success but carries no data.
pub type EmptyResult = JSONRPCResult;
