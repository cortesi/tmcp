#![allow(missing_docs)]

use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ElicitRequestURLParams, TaskMetadata};
use crate::macros::with_meta;

pub const PREVIOUS_PROTOCOL_VERSION: &str = "2025-06-18";
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
/// JSON-RPC protocol version string.
pub(crate) const JSONRPC_VERSION: &str = "2.0";

/// Refers to any valid JSON-RPC object that can be decoded off the wire, or
/// encoded to be sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JSONRPCMessage {
    Request(JSONRPCRequest),
    Notification(JSONRPCNotification),
    Response(JSONRPCResponse),
}

/// A progress token, used to associate progress notifications with the original
/// request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgressToken {
    String(String),
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

/// Common params for any task-augmented request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskAugmentedRequestParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,
    #[serde(flatten)]
    pub params: RequestParams,
}

/// Common params for any request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestMeta {
    /// If specified, the caller is requesting out-of-band progress
    /// notifications for this request (as represented by
    /// notifications/progress). The value of this parameter is an opaque token
    /// that will be attached to any subsequent notifications. The receiver is
    /// not obligated to provide these notifications.
    #[serde(rename = "progressToken", skip_serializing_if = "Option::is_none")]
    pub progress_token: Option<ProgressToken>,
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub method: String,
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
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<NotificationParams>,
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRpcResult {
    /// This result property is reserved by the protocol to allow clients and
    /// servers to attach additional metadata to their responses.
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

/// Alias for JSON-RPC result payloads.
pub type Result = JSONRpcResult;

/// A uniquely identifying ID for a request in JSON-RPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    /// String request ID.
    String(String),
    /// Numeric request ID.
    Number(i64),
}

impl RequestId {
    /// Convert the request ID to a string key for internal tracking.
    ///
    /// This normalizes both string and numeric IDs into a consistent string format
    /// that can be used as a hash map key.
    pub fn to_key(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Number(n) => format!("__num__{n}"),
        }
    }
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
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(flatten)]
    pub request: Request,
}

/// A notification which does not expect a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCNotification {
    pub jsonrpc: String,
    #[serde(flatten)]
    pub notification: Notification,
}

/// A successful (non-error) response to a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCResultResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    pub result: Result,
}

/// A response to a request that indicates an error occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCErrorResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    pub error: ErrorObject,
}

/// A response to a request, containing either the result or error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JSONRPCResponse {
    Result(JSONRPCResultResponse),
    Error(JSONRPCErrorResponse),
}

// Standard JSON-RPC error codes
/// JSON-RPC parse error code.
pub(crate) const PARSE_ERROR: i32 = -32700;
/// JSON-RPC invalid request error code.
pub(crate) const INVALID_REQUEST: i32 = -32600;
/// JSON-RPC method not found error code.
pub(crate) const METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC invalid params error code.
pub(crate) const INVALID_PARAMS: i32 = -32602;
/// JSON-RPC internal error code.
pub(crate) const INTERNAL_ERROR: i32 = -32603;

/// Implementation-specific JSON-RPC error code indicating URL elicitation is required.
pub const URL_ELICITATION_REQUIRED: i32 = -32042;

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

/// An error response that indicates that the server requires the client to
/// provide additional information via an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct URLElicitationRequiredError {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    pub error: URLElicitationRequiredErrorObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct URLElicitationRequiredErrorObject {
    pub code: i32,
    pub message: String,
    pub data: URLElicitationRequiredData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct URLElicitationRequiredData {
    pub elicitations: Vec<ElicitRequestURLParams>,
    #[serde(flatten)]
    pub other: HashMap<String, Value>,
}

// Empty result
/// A response that indicates success but carries no data.
pub(crate) type EmptyResult = Result;
